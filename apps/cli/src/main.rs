use api_client_core::{
    ApiKeyPlacement, HeaderEntry, KeyValueEntry, MultipartPart, MultipartValue, RequestAuth,
    RequestBody, RequestDefinition, RequestOptions,
};
use api_client_http::HttpExecutor;
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(name = "apic", version, about = "Reqglide local-first API client CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Send one HTTP request and print a JSON response snapshot.
    Send {
        url: String,
        #[arg(short, long, default_value = "GET")]
        method: String,
        #[arg(short = 'H', long = "header", value_parser = parse_header, help = "Add a request header as 'Name: value'; repeatable")]
        headers: Vec<HeaderEntry>,
        #[arg(short = 'q', long = "query", value_parser = parse_key_value, help = "Add a query parameter as 'name=value'; repeatable")]
        query_params: Vec<KeyValueEntry>,
        #[arg(short, long, conflicts_with_all = ["form_fields", "multipart_fields", "multipart_files", "binary_file"], help = "Send a raw request body")]
        body: Option<String>,
        #[arg(long = "form", value_parser = parse_key_value, conflicts_with_all = ["body", "multipart_fields", "multipart_files", "binary_file"], help = "Add a URL-encoded form field as 'name=value'; repeatable")]
        form_fields: Vec<KeyValueEntry>,
        #[arg(long = "multipart", value_parser = parse_key_value, conflicts_with_all = ["body", "form_fields", "binary_file"], help = "Add a multipart text field as 'name=value'; repeatable")]
        multipart_fields: Vec<KeyValueEntry>,
        #[arg(long = "multipart-file", value_parser = parse_key_value, conflicts_with_all = ["body", "form_fields", "binary_file"], value_name = "NAME=PATH", help = "Add a multipart local file; repeatable")]
        multipart_files: Vec<KeyValueEntry>,
        #[arg(long, conflicts_with_all = ["body", "form_fields", "multipart_fields", "multipart_files"], value_name = "PATH", help = "Stream a local file as the request body")]
        binary_file: Option<String>,
        #[arg(long, help = "Content type used with --body or --binary-file")]
        content_type: Option<String>,
        #[arg(long, conflicts_with_all = ["bearer", "api_key"], value_name = "USER:PASSWORD", help = "Use HTTP Basic authentication")]
        basic_auth: Option<String>,
        #[arg(long, conflicts_with_all = ["basic_auth", "api_key"], help = "Use Bearer token authentication")]
        bearer: Option<String>,
        #[arg(long, value_parser = parse_key_value, conflicts_with_all = ["basic_auth", "bearer"], value_name = "NAME=VALUE", help = "Use API-key authentication")]
        api_key: Option<KeyValueEntry>,
        #[arg(long, value_enum, default_value_t = CliApiKeyPlacement::Header, help = "Place --api-key in a header or query parameter")]
        api_key_in: CliApiKeyPlacement,
        #[arg(long, default_value_t = 30_000)]
        timeout_ms: u64,
        #[arg(long, default_value_t = 100 * 1024 * 1024)]
        max_request_bytes: u64,
        #[arg(long, default_value_t = 10 * 1024 * 1024)]
        max_response_bytes: u64,
        #[arg(long)]
        no_follow_redirects: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliApiKeyPlacement {
    Header,
    Query,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Command::Send {
            url,
            method,
            headers,
            query_params,
            body,
            form_fields,
            multipart_fields,
            multipart_files,
            binary_file,
            content_type,
            basic_auth,
            bearer,
            api_key,
            api_key_in,
            timeout_ms,
            max_request_bytes,
            max_response_bytes,
            no_follow_redirects,
        } => {
            let body = if let Some(path) = binary_file {
                RequestBody::Binary {
                    path,
                    content_type: Some(
                        content_type.unwrap_or_else(|| "application/octet-stream".to_owned()),
                    ),
                }
            } else if !multipart_fields.is_empty() || !multipart_files.is_empty() {
                let parts = multipart_fields
                    .into_iter()
                    .map(|field| MultipartPart {
                        name: field.name,
                        enabled: field.enabled,
                        value: MultipartValue::Text {
                            content: field.value,
                            content_type: None,
                        },
                    })
                    .chain(multipart_files.into_iter().map(|field| MultipartPart {
                        name: field.name,
                        enabled: field.enabled,
                        value: MultipartValue::File {
                            path: field.value,
                            file_name: None,
                            content_type: None,
                        },
                    }))
                    .collect();
                RequestBody::Multipart { parts }
            } else if form_fields.is_empty() {
                body.map_or(RequestBody::Empty, |content| RequestBody::Text {
                    content,
                    content_type: Some(
                        content_type.unwrap_or_else(|| "application/json".to_owned()),
                    ),
                })
            } else {
                RequestBody::FormUrlEncoded {
                    fields: form_fields,
                }
            };
            let auth = if let Some(credentials) = basic_auth {
                let (username, password) =
                    credentials.split_once(':').unwrap_or((&credentials, ""));
                RequestAuth::Basic {
                    username: username.to_owned(),
                    password: password.to_owned(),
                }
            } else if let Some(token) = bearer {
                RequestAuth::Bearer { token }
            } else if let Some(api_key) = api_key {
                RequestAuth::ApiKey {
                    key: api_key.name,
                    value: api_key.value,
                    placement: match api_key_in {
                        CliApiKeyPlacement::Header => ApiKeyPlacement::Header,
                        CliApiKeyPlacement::Query => ApiKeyPlacement::Query,
                    },
                }
            } else {
                RequestAuth::None
            };
            let request = RequestDefinition {
                method,
                url,
                query_params,
                headers,
                auth,
                body,
                options: RequestOptions {
                    timeout_ms,
                    max_request_bytes,
                    max_response_bytes,
                    follow_redirects: !no_follow_redirects,
                },
            };

            HttpExecutor.execute(request).await
        }
    };

    match result {
        Ok(response) => match serde_json::to_string_pretty(&response) {
            Ok(json) => println!("{json}"),
            Err(error) => {
                eprintln!("could not serialize response: {error}");
                std::process::exit(1);
            }
        },
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

fn parse_header(value: &str) -> Result<HeaderEntry, String> {
    let Some((name, value)) = value.split_once(':') else {
        return Err("headers must use the form 'Name: value'".to_owned());
    };

    if name.trim().is_empty() {
        return Err("header name cannot be empty".to_owned());
    }

    Ok(HeaderEntry {
        name: name.trim().to_owned(),
        value: value.trim().to_owned(),
        enabled: true,
    })
}

fn parse_key_value(value: &str) -> Result<KeyValueEntry, String> {
    let Some((name, value)) = value.split_once('=') else {
        return Err("values must use the form 'name=value'".to_owned());
    };
    if name.is_empty() {
        return Err("name cannot be empty".to_owned());
    }
    Ok(KeyValueEntry {
        name: name.to_owned(),
        value: value.to_owned(),
        enabled: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_query_form_and_auth_options() {
        let cli = Cli::try_parse_from([
            "apic",
            "send",
            "https://example.com/token",
            "--query",
            "tenant=local",
            "--form",
            "grant_type=client_credentials",
            "--api-key",
            "X-API-Key={{apiKey}}",
            "--api-key-in",
            "header",
        ])
        .unwrap();

        let Command::Send {
            query_params,
            form_fields,
            multipart_fields,
            api_key,
            ..
        } = cli.command;
        assert_eq!(query_params[0].name, "tenant");
        assert_eq!(form_fields[0].value, "client_credentials");
        assert!(multipart_fields.is_empty());
        assert_eq!(api_key.unwrap().name, "X-API-Key");
    }

    #[test]
    fn parses_multipart_and_binary_file_options() {
        let multipart = Cli::try_parse_from([
            "apic",
            "send",
            "https://example.com/upload",
            "--multipart",
            "description=report",
            "--multipart-file",
            "document=report.pdf",
        ]);
        assert!(multipart.is_ok());

        let binary = Cli::try_parse_from([
            "apic",
            "send",
            "https://example.com/upload",
            "--binary-file",
            "archive.zip",
            "--form",
            "mode=fast",
        ]);
        assert!(binary.is_err());
    }

    #[test]
    fn refuses_multiple_auth_modes() {
        assert!(
            Cli::try_parse_from([
                "apic",
                "send",
                "https://example.com",
                "--bearer",
                "token",
                "--basic-auth",
                "user:password",
            ])
            .is_err()
        );
    }
}
