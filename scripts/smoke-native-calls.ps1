param([switch]$Keep)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('axonyx-native-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path (Join-Path $fixture 'src') -Force | Out-Null
try {
    Push-Location $root
    try {
        $metadata = & cargo metadata --no-deps --format-version 1 --locked
        if ($LASTEXITCODE -ne 0) { throw 'Unable to locate Cargo target cache' }
        $target = ($metadata | ConvertFrom-Json).target_directory
        $module = & cargo run -p axonyx-runtime --example native_calls_codegen --locked --quiet
        if ($LASTEXITCODE -ne 0) { throw 'Native fixture generation failed' }
    } finally { Pop-Location }
    $module | Set-Content -LiteralPath (Join-Path $fixture 'src/backend.rs') -Encoding utf8
    $dependency = (Join-Path $root 'crates/axonyx-runtime').Replace('\', '/')
    @"
[package]
name = "axonyx-native-consumer"
version = "0.0.0"
edition = "2021"
[workspace]
[dependencies]
axonyx-runtime = { path = "$dependency" }
axonyx-core = { path = "${dependency}/../axonyx-core" }
serde = { version = "1.0.228", features = ["derive"] }
serde_json = "1.0.145"
"@ | Set-Content -LiteralPath (Join-Path $fixture 'Cargo.toml') -Encoding utf8
    Copy-Item -LiteralPath (Join-Path $root 'crates/axonyx-runtime/tests/fixtures/native-consumer.rs') -Destination (Join-Path $fixture 'src/main.rs')
    Copy-Item -LiteralPath (Join-Path $root 'crates/axonyx-runtime/tests/fixtures/native-backend.ax') -Destination (Join-Path $fixture 'src/native-backend.ax')
    Copy-Item -LiteralPath (Join-Path $root 'Cargo.lock') -Destination (Join-Path $fixture 'Cargo.lock')
    & cargo check --manifest-path (Join-Path $fixture 'Cargo.toml') --target-dir $target --offline --quiet
    if ($LASTEXITCODE -ne 0) { throw 'Native fixture dependency resolution failed' }
    & cargo run --manifest-path (Join-Path $fixture 'Cargo.toml') --target-dir $target --locked --quiet
    if ($LASTEXITCODE -ne 0) { throw 'Compiled native/preview parity failed' }
} finally {
    if (-not $Keep) {
        $resolved = [IO.Path]::GetFullPath($fixture)
        $temp = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
        if (-not $resolved.StartsWith($temp, [StringComparison]::OrdinalIgnoreCase) -or (Split-Path $resolved -Leaf) -notmatch '^axonyx-native-[a-f0-9]{32}$') {
            throw 'Refusing cleanup outside the generated temporary fixture'
        }
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
}
