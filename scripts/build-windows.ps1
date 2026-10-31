[CmdletBinding()]
param(
    [switch]$Preview,
    [switch]$CommunityPreview,
    [switch]$SkipBuild,
    [string]$CertificateThumbprint,
    [string]$TimestampUrl
)

$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'Windows packaging must run on Windows.' }
if ($Preview -and $CommunityPreview) { throw 'Choose one preview channel.' }
if ($SkipBuild -and -not $Preview) { throw '-SkipBuild is permitted only for internal preview inspection.' }
$unsignedPreview = $Preview -or $CommunityPreview

$projectRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
Push-Location -LiteralPath $projectRoot
try {
    $tauriConfig = Get-Content -LiteralPath 'apps/desktop/src-tauri/tauri.conf.json' -Raw | ConvertFrom-Json
    $desktopPackage = Get-Content -LiteralPath 'apps/desktop/package.json' -Raw | ConvertFrom-Json
    $version = [string]$tauriConfig.version
    if ($version -notmatch '^\d+\.\d+\.\d+(?:-[a-zA-Z0-9.-]+)?$') { throw 'A concrete semantic version is required.' }
    $cargoSource = Get-Content -LiteralPath 'Cargo.toml' -Raw
    $cargoVersion = [regex]::Match($cargoSource, '(?m)^version\s*=\s*"([^"]+)"\s*\r?$').Groups[1].Value
    if ($version -ne $desktopPackage.version -or $version -ne $cargoVersion) {
        throw 'Cargo, desktop package, and Tauri versions must match.'
    }

    # The desktop sandbox and host runner can own this checkout under different
    # Windows identities. Trust only this explicit path for these read commands;
    # do not alter the user's global Git configuration.
    $gitSafeRoot = $projectRoot.Replace('\', '/')
    $commit = (& git -c "safe.directory=$gitSafeRoot" rev-parse --verify HEAD 2>$null)
    if ($LASTEXITCODE -ne 0) { $commit = $null }
    $dirtyFiles = @(& git -c "safe.directory=$gitSafeRoot" status --porcelain --untracked-files=all)
    if ($LASTEXITCODE -ne 0) { throw 'Could not inspect source-control status.' }
    $sourceClean = $null -ne $commit -and $dirtyFiles.Count -eq 0
    if ($CommunityPreview -and -not $sourceClean) { throw 'A community preview requires clean, committed release source.' }
    if ($CommunityPreview -and (-not (Test-Path -LiteralPath 'LICENSE') -or $cargoSource -notmatch '(?m)^license\s*=\s*"MIT"')) {
        throw 'A community preview must include the project MIT license.'
    }

    $signingConfigPath = $null
    if (-not $unsignedPreview) {
        if (-not $sourceClean) { throw 'A signed candidate requires a clean, committed source checkout.' }
        $CertificateThumbprint = $CertificateThumbprint -replace '\s', ''
        if ($CertificateThumbprint -notmatch '^[a-fA-F0-9]{40}$') { throw 'A Windows code-signing certificate thumbprint is required.' }
        $certificate = Get-Item -LiteralPath "Cert:\CurrentUser\My\$CertificateThumbprint"
        if (-not $certificate.HasPrivateKey -or $certificate.NotAfter -le (Get-Date)) { throw 'The signing certificate must have a private key and must not be expired.' }
        $timestampUri = $null
        if (-not [uri]::TryCreate($TimestampUrl, [UriKind]::Absolute, [ref]$timestampUri) -or $timestampUri.Scheme -notin @('https', 'http') -or $timestampUri.UserInfo) {
            throw 'Provide the timestamp URL supplied by your signing provider.'
        }
        $signingConfigPath = Join-Path $projectRoot '.tools/windows-signing.json'
        New-Item -ItemType Directory -Path (Split-Path -Parent $signingConfigPath) -Force | Out-Null
        @{ bundle = @{ windows = @{ certificateThumbprint = $CertificateThumbprint; digestAlgorithm = 'sha256'; timestampUrl = $TimestampUrl } } } |
            ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $signingConfigPath -Encoding utf8
    }

    try {
        if (-not $SkipBuild) {
            $buildArguments = @('--filter', '@local-api/desktop', 'tauri', 'build', '--ci', '--bundles', 'nsis,msi')
            if ($signingConfigPath) { $buildArguments += @('--config', $signingConfigPath) }
            & pnpm @buildArguments
            if ($LASTEXITCODE -ne 0) { throw 'Windows installer build failed.' }
        }
    } finally {
        if ($signingConfigPath -and (Test-Path -LiteralPath $signingConfigPath)) { Remove-Item -LiteralPath $signingConfigPath }
    }

    $postBuildCommit = (& git -c "safe.directory=$gitSafeRoot" rev-parse --verify HEAD 2>$null)
    if ($LASTEXITCODE -ne 0) { $postBuildCommit = $null }
    $postBuildChanges = @(& git -c "safe.directory=$gitSafeRoot" status --porcelain --untracked-files=all)
    if ($LASTEXITCODE -ne 0) { throw 'Could not inspect source status after building.' }
    $sourceClean = $sourceClean -and $postBuildCommit -eq $commit -and $postBuildChanges.Count -eq 0
    if (-not $Preview -and -not $sourceClean) { throw 'Source changed while building; the candidate cannot be promoted.' }

    $expected = @(
        @{ Kind = 'nsis'; Path = Join-Path $projectRoot ("target/release/bundle/nsis/{0}_{1}_x64-setup.exe" -f $tauriConfig.productName, $version); Name = "reqglide_${version}_windows-x64-setup.exe" },
        @{ Kind = 'msi'; Path = Join-Path $projectRoot ("target/release/bundle/msi/{0}_{1}_x64_en-US.msi" -f $tauriConfig.productName, $version); Name = "reqglide_${version}_windows-x64.msi" }
    )
    $verified = @()
    foreach ($installer in $expected) {
        if (-not (Test-Path -LiteralPath $installer.Path -PathType Leaf)) { throw "Missing $($installer.Kind) installer." }
        $signature = Get-AuthenticodeSignature -LiteralPath $installer.Path
        if (-not $unsignedPreview) {
            if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Thumbprint -ne $CertificateThumbprint -or -not $signature.TimeStamperCertificate) {
                throw "The $($installer.Kind) installer must have a valid, timestamped signature from the configured publisher."
            }
        }
        $verified += @{ Installer = $installer; Signature = $signature }
    }

    $outputDirectory = Join-Path $projectRoot ".release-artifacts/windows/$version"
    New-Item -ItemType Directory -Path $outputDirectory -Force | Out-Null
    $assets = foreach ($item in $verified) {
        $destination = Join-Path $outputDirectory $item.Installer.Name
        Copy-Item -LiteralPath $item.Installer.Path -Destination $destination -Force
        $file = Get-Item -LiteralPath $destination
        [ordered]@{
            file = $file.Name
            kind = $item.Installer.Kind
            bytes = $file.Length
            sha256 = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant()
            signatureStatus = [string]$item.Signature.Status
        }
    }
    $manifest = [ordered]@{
        schemaVersion = 1
        product = $tauriConfig.productName
        version = $version
        platform = 'windows-x64'
        distributionClass = $(if ($Preview) { 'internal-preview' } elseif ($CommunityPreview) { 'community-preview' } else { 'signed-candidate' })
        sourceCommit = $commit
        sourceClean = $sourceClean
        buildPerformed = -not $SkipBuild
        cargoLockSha256 = (Get-FileHash -LiteralPath 'Cargo.lock' -Algorithm SHA256).Hash.ToLowerInvariant()
        pnpmLockSha256 = (Get-FileHash -LiteralPath 'pnpm-lock.yaml' -Algorithm SHA256).Hash.ToLowerInvariant()
        generatedAtUtc = [DateTime]::UtcNow.ToString('o')
        assets = @($assets)
    }
    $manifest | ConvertTo-Json -Depth 7 | Set-Content -LiteralPath (Join-Path $outputDirectory 'manifest.json') -Encoding utf8
    @($assets | ForEach-Object { "$($_.sha256)  $($_.file)" }) | Set-Content -LiteralPath (Join-Path $outputDirectory 'SHA256SUMS.txt') -Encoding ascii
    Write-Output "Prepared $($manifest.distributionClass) artifacts in $outputDirectory"
    if ($Preview) { Write-Output 'Internal preview only. This does not establish public-release readiness.' }
    if ($CommunityPreview) { Write-Output 'Unsigned community preview for evaluation. This is not a production release.' }
} finally {
    Pop-Location
}
