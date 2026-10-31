//! Windows PAC/WPAD support missing from reqwest's registry-only system matcher.
//! Native calls run on a blocking worker and never receive request credentials.

use super::{HttpExecutionContext, HttpExecutionError, ProxyConfiguration};
use std::{ffi::c_void, ptr, time::Duration};
use windows_sys::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, GetLastError, GlobalFree},
    Networking::WinHttp::*,
};

#[derive(Debug, PartialEq, Eq)]
enum Route {
    Unconfigured,
    Direct,
    Proxy(String, String),
}

pub(super) async fn prepare_context(
    mut context: HttpExecutionContext,
    url: &reqwest::Url,
    timeout_ms: u64,
) -> Result<HttpExecutionContext, HttpExecutionError> {
    if !matches!(context.proxy, ProxyConfiguration::System) || has_environment_proxy(url.scheme()) {
        return Ok(context);
    }
    // Local requests must keep working offline without consulting an enterprise PAC server.
    if url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    }) {
        return Ok(context);
    }
    // Resolve by origin so query API keys, paths, and other request data never
    // reach PAC code. Networks requiring path-specific rules can use Custom proxy.
    let destination = url.origin().ascii_serialization();
    let budget = timeout_ms.clamp(1, 10_000);
    let route = tokio::time::timeout(
        Duration::from_millis(budget),
        tokio::task::spawn_blocking(move || resolve(&destination, budget)),
    )
    .await
    .map_err(|_| HttpExecutionError::SystemProxyTimeout)?
    .map_err(|_| HttpExecutionError::SystemProxyLookup { code: 0 })??;
    context.proxy = match route {
        Route::Unconfigured => ProxyConfiguration::System,
        Route::Direct => ProxyConfiguration::Disabled,
        Route::Proxy(url, bypass) => {
            let environment_bypass = std::env::var("no_proxy")
                .or_else(|_| std::env::var("NO_PROXY"))
                .unwrap_or_default();
            ProxyConfiguration::Custom {
                url,
                username: String::new(),
                password: String::new(),
                no_proxy: [bypass, environment_bypass]
                    .into_iter()
                    .filter(|value| !value.is_empty())
                    .collect::<Vec<_>>()
                    .join(","),
            }
        }
    };
    Ok(context)
}

fn has_environment_proxy(scheme: &str) -> bool {
    let keys = if scheme == "https" {
        ["https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"]
    } else {
        ["http_proxy", "HTTP_PROXY", "all_proxy", "ALL_PROXY"]
    };
    keys.into_iter()
        .any(|key| std::env::var_os(key).is_some_and(|value| !value.is_empty()))
}

struct Settings(WINHTTP_CURRENT_USER_IE_PROXY_CONFIG);
impl Drop for Settings {
    fn drop(&mut self) {
        for pointer in [
            self.0.lpszAutoConfigUrl,
            self.0.lpszProxy,
            self.0.lpszProxyBypass,
        ] {
            if !pointer.is_null() {
                // WinHttpGetIEProxyConfigForCurrentUser transfers these GlobalAlloc strings.
                unsafe {
                    GlobalFree(pointer.cast());
                }
            }
        }
    }
}

struct ProxyInfo(WINHTTP_PROXY_INFO);
impl Drop for ProxyInfo {
    fn drop(&mut self) {
        for pointer in [self.0.lpszProxy, self.0.lpszProxyBypass] {
            if !pointer.is_null() {
                // WinHttpGetProxyForUrl transfers these GlobalAlloc strings.
                unsafe {
                    GlobalFree(pointer.cast());
                }
            }
        }
    }
}

struct Session(*mut c_void);
impl Drop for Session {
    fn drop(&mut self) {
        // This guard owns the session returned by WinHttpOpen.
        unsafe {
            WinHttpCloseHandle(self.0);
        }
    }
}

fn resolve(destination: &str, timeout_ms: u64) -> Result<Route, HttpExecutionError> {
    let mut settings = Settings(WINHTTP_CURRENT_USER_IE_PROXY_CONFIG::default());
    // The output is initialized and stays alive until all API-owned strings are released.
    if unsafe { WinHttpGetIEProxyConfigForCurrentUser(&mut settings.0) } == 0 {
        // An account without Internet Options has no PAC configuration; reqwest can
        // still use its usual environment/manual proxy behavior.
        return settings_lookup_failure(unsafe { GetLastError() });
    }
    let explicit_pac = !settings.0.lpszAutoConfigUrl.is_null();
    if !explicit_pac && settings.0.fAutoDetect == 0 {
        return Ok(Route::Unconfigured);
    }
    let agent: Vec<u16> = "Reqglide".encode_utf16().chain(Some(0)).collect();
    // WinHTTP is used only for proxy discovery, not for the API request itself.
    let handle = unsafe {
        WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_NO_PROXY,
            ptr::null(),
            ptr::null(),
            0,
        )
    };
    if handle.is_null() {
        return Err(HttpExecutionError::SystemProxyLookup {
            code: unsafe { GetLastError() },
        });
    }
    let session = Session(handle);
    let milliseconds = timeout_ms.min(i32::MAX as u64) as i32;
    if unsafe {
        WinHttpSetTimeouts(
            session.0,
            milliseconds,
            milliseconds,
            milliseconds,
            milliseconds,
        )
    } == 0
    {
        return Err(HttpExecutionError::SystemProxyLookup {
            code: unsafe { GetLastError() },
        });
    }
    let mut options = WINHTTP_AUTOPROXY_OPTIONS {
        dwFlags: if explicit_pac {
            WINHTTP_AUTOPROXY_CONFIG_URL
        } else {
            WINHTTP_AUTOPROXY_AUTO_DETECT
        },
        dwAutoDetectFlags: if explicit_pac {
            0
        } else {
            WINHTTP_AUTO_DETECT_TYPE_DHCP | WINHTTP_AUTO_DETECT_TYPE_DNS_A
        },
        lpszAutoConfigUrl: settings.0.lpszAutoConfigUrl,
        // Do not silently send Windows credentials while downloading a PAC file.
        fAutoLogonIfChallenged: 0,
        ..Default::default()
    };
    let destination: Vec<u16> = destination.encode_utf16().chain(Some(0)).collect();
    let mut info = ProxyInfo(WINHTTP_PROXY_INFO::default());
    if unsafe { WinHttpGetProxyForUrl(session.0, destination.as_ptr(), &mut options, &mut info.0) }
        == 0
    {
        let code = unsafe { GetLastError() };
        // Automatic detection can legitimately find no PAC on a direct network.
        if !explicit_pac && code == ERROR_WINHTTP_AUTODETECTION_FAILED {
            return Ok(Route::Unconfigured);
        }
        return Err(HttpExecutionError::SystemProxyLookup { code });
    }
    if info.0.dwAccessType == WINHTTP_ACCESS_TYPE_NO_PROXY {
        return Ok(Route::Direct);
    }
    if info.0.dwAccessType != WINHTTP_ACCESS_TYPE_NAMED_PROXY {
        return Err(HttpExecutionError::SystemProxyLookup { code: 0 });
    }
    let proxy = read_wide(info.0.lpszProxy)?;
    let bypass = read_wide(info.0.lpszProxyBypass)?.replace([';', ' '], ",");
    parse_proxy(&proxy, &bypass)
}

fn settings_lookup_failure(code: u32) -> Result<Route, HttpExecutionError> {
    if code == ERROR_FILE_NOT_FOUND {
        Ok(Route::Unconfigured)
    } else {
        // A failed lookup must not masquerade as a successful direct-route decision.
        Err(HttpExecutionError::SystemProxyLookup { code })
    }
}

fn read_wide(pointer: *const u16) -> Result<String, HttpExecutionError> {
    if pointer.is_null() {
        return Ok(String::new());
    }
    // WinHTTP documents a null-terminated UTF-16 allocation. Bound traversal as
    // well, and never expose the native proxy/PAC strings in an error message.
    for length in 0..65_536 {
        if unsafe { *pointer.add(length) } == 0 {
            return Ok(String::from_utf16_lossy(unsafe {
                std::slice::from_raw_parts(pointer, length)
            }));
        }
    }
    Err(HttpExecutionError::SystemProxyLookup { code: 0 })
}

fn parse_proxy(value: &str, bypass: &str) -> Result<Route, HttpExecutionError> {
    // WinHTTP returns an HTTP proxy address (possibly a prioritized list), not
    // JavaScript or a raw PAC result. Use the first configured proxy only.
    let proxy = value
        .split(';')
        .find(|item| !item.trim().is_empty())
        .unwrap_or_default()
        .trim();
    if proxy.eq_ignore_ascii_case("DIRECT") {
        return Ok(Route::Direct);
    }
    let proxy = if proxy.contains("://") {
        proxy.to_owned()
    } else {
        format!("http://{proxy}")
    };
    let parsed = reqwest::Url::parse(&proxy)
        .map_err(|_| HttpExecutionError::SystemProxyLookup { code: 0 })?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(HttpExecutionError::SystemProxyLookup { code: 0 });
    }
    Ok(Route::Proxy(proxy, bypass.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinguishes_missing_settings_from_a_failed_windows_lookup() {
        assert_eq!(
            settings_lookup_failure(ERROR_FILE_NOT_FOUND).unwrap(),
            Route::Unconfigured
        );
        assert!(matches!(
            settings_lookup_failure(5),
            Err(HttpExecutionError::SystemProxyLookup { code: 5 })
        ));
    }

    #[test]
    fn uses_native_http_proxy_and_preserves_bypass_rules() {
        assert_eq!(
            parse_proxy(
                "proxy.example.com:8080;backup.example.com:8080",
                "localhost,.internal"
            )
            .unwrap(),
            Route::Proxy(
                "http://proxy.example.com:8080".to_owned(),
                "localhost,.internal".to_owned()
            )
        );
        assert_eq!(parse_proxy("DIRECT", "").unwrap(), Route::Direct);
        assert_eq!(
            parse_proxy("https://proxy.example.com:8443", "").unwrap(),
            Route::Proxy("https://proxy.example.com:8443".to_owned(), String::new())
        );
    }

    #[test]
    fn rejects_invalid_proxy_results_without_exposing_values() {
        for value in [
            "",
            "user:secret@proxy.example.com:8080",
            "file:///private",
            "http://",
        ] {
            let error = parse_proxy(value, "").unwrap_err().to_string();
            assert!(!error.contains("secret"));
            assert!(!error.contains("private"));
        }
    }

    #[tokio::test]
    async fn explicit_proxy_settings_and_loopback_do_not_consult_windows_pac() {
        let url = reqwest::Url::parse("http://127.0.0.1:12345").unwrap();
        let context = prepare_context(HttpExecutionContext::default(), &url, 1)
            .await
            .unwrap();
        assert!(matches!(context.proxy, ProxyConfiguration::System));
        let context = prepare_context(
            HttpExecutionContext {
                proxy: ProxyConfiguration::Disabled,
                ..Default::default()
            },
            &reqwest::Url::parse("https://example.com").unwrap(),
            1,
        )
        .await
        .unwrap();
        assert!(matches!(context.proxy, ProxyConfiguration::Disabled));
    }
}
