param()
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('axonyx-msrv-' + [guid]::NewGuid().ToString('N'))
try {
    New-Item -ItemType Directory -Path (Join-Path $fixture 'src') | Out-Null
    $runtime = (Join-Path $root 'crates/axonyx-runtime').Replace('\', '/')
    @"
[package]
name = "axonyx-msrv-consumer"
version = "0.0.0"
edition = "2021"
rust-version = "1.82"
[workspace]
resolver = "2"
[dependencies]
axonyx-runtime = { path = "$runtime" }
[features]
axum = ["axonyx-runtime/axum", "axonyx-runtime/storage"]
tokio = ["axonyx-runtime/tokio"]
"@ | Set-Content -LiteralPath (Join-Path $fixture 'Cargo.toml') -Encoding utf8
    'fn main() { let _ = axonyx_runtime::backend::AxEnv::default(); }' |
        Set-Content -LiteralPath (Join-Path $fixture 'src/main.rs') -Encoding utf8
    Push-Location $root
    try {
        $metadata = & cargo metadata --no-deps --format-version 1
        if ($LASTEXITCODE -ne 0) { throw 'Cannot locate the Cargo check cache' }
        $target = ($metadata | ConvertFrom-Json).target_directory
    } finally { Pop-Location }
    $manifest = Join-Path $fixture 'Cargo.toml'
    & cargo generate-lockfile --manifest-path $manifest
    if ($LASTEXITCODE -ne 0) { throw 'Fresh consumer resolution failed' }
    $lock = Get-Content -LiteralPath (Join-Path $fixture 'Cargo.lock') -Raw
    if ($lock -notmatch '(?m)^name = "tokio"\r?\nversion = "([^"]+)"') {
        throw 'Fresh consumer did not resolve Tokio'
    }
    $version = [version]$Matches[1]
    if ($version -lt [version]'1.52.3' -or $version -ge [version]'1.54.0') {
        throw "Fresh consumer escaped the Tokio MSRV range: $version"
    }
    & cargo check --manifest-path $manifest --locked --target-dir $target
    if ($LASTEXITCODE -ne 0) { throw 'Default runtime consumer check failed' }
    foreach ($feature in @('tokio', 'axum')) {
        & cargo check --manifest-path $manifest --locked --target-dir $target --features $feature
        if ($LASTEXITCODE -ne 0) { throw "Runtime consumer feature $feature failed" }
    }
    Write-Host "Fresh runtime consumer passed: default/Tokio/Axum, resolved Tokio $version."
} finally {
    $resolved = [IO.Path]::GetFullPath($fixture)
    $temp = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if (-not $resolved.StartsWith($temp, [StringComparison]::OrdinalIgnoreCase) -or (Split-Path $resolved -Leaf) -notmatch '^axonyx-msrv-[a-f0-9]{32}$') {
        throw 'Refusing cleanup outside the generated MSRV fixture'
    }
    if (Test-Path -LiteralPath $resolved) { Remove-Item -LiteralPath $resolved -Recurse -Force }
}
