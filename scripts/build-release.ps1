param([switch]$Personal, [switch]$Installers, [switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path $PSScriptRoot -Parent
Push-Location $projectRoot
try {
    if (-not $SkipBuild) {
        # Keep local Windows usernames and dependency paths out of binaries.
        # Encoded flags preserve profile paths that contain spaces.
        $previousEncodedFlags = $env:CARGO_ENCODED_RUSTFLAGS
        $separator = [char]31
        $buildFlags = if ($previousEncodedFlags) { $previousEncodedFlags } else { ($env:RUSTFLAGS -split '\s+' | Where-Object { $_ }) -join $separator }
        $profileRoot = [Environment]::GetFolderPath('UserProfile')
        $remap = '--remap-path-prefix=' + $profileRoot + '=/build'
        $env:CARGO_ENCODED_RUSTFLAGS = if ($buildFlags) { $buildFlags + $separator + $remap } else { $remap }
        $tauriArgs = @('node_modules/@tauri-apps/cli/tauri.js', 'build')
        if (-not $Installers) { $tauriArgs += '--no-bundle' }
        try {
            & node @tauriArgs
            if ($LASTEXITCODE -ne 0) { throw "Build failed ($LASTEXITCODE)." }
        } finally {
            if ($null -eq $previousEncodedFlags) { Remove-Item Env:CARGO_ENCODED_RUSTFLAGS -ErrorAction SilentlyContinue }
            else { $env:CARGO_ENCODED_RUSTFLAGS = $previousEncodedFlags }
        }
    }
    $name = if ($Personal) { 'Twitch Timeshift Personal' } else { 'Twitch Timeshift' }
    $dest = Join-Path $projectRoot "release/$name"
    New-Item -ItemType Directory -Force -Path $dest | Out-Null
    Copy-Item -LiteralPath 'src-tauri/target/release/twitch-timeshift.exe' -Destination (Join-Path $dest 'Twitch Timeshift.exe') -Force
    Set-Content -LiteralPath (Join-Path $dest 'portable.txt') -Value 'Settings are stored in data beside this executable.' -Encoding ASCII
    if ($Personal) {
        $secretPath = Join-Path $projectRoot '.twitch-secrets.json'
        if (-not (Test-Path -LiteralPath $secretPath)) { throw 'Personal packaging requires .twitch-secrets.json.' }
        $dataDir = Join-Path $dest 'data'
        New-Item -ItemType Directory -Force -Path $dataDir | Out-Null
        Copy-Item -LiteralPath $secretPath -Destination (Join-Path $dataDir 'twitch-secrets.json') -Force
        $personalConfig = Join-Path $projectRoot '.personal-config.json'
        if ((Test-Path -LiteralPath $personalConfig) -and -not (Test-Path -LiteralPath (Join-Path $dataDir 'config.json'))) {
            Copy-Item -LiteralPath $personalConfig -Destination (Join-Path $dataDir 'config.json')
        }
    } else {
        $personalFiles = @(Get-ChildItem -LiteralPath $dest -Recurse -File | Where-Object { $_.Name -match 'secrets|config\.json' })
        if ($personalFiles.Count -gt 0) { throw 'Public output contains runtime settings. Use a fresh public output directory.' }
    }
    @"
Double-click Twitch Timeshift.exe. Move this entire folder to your Apps folder.
Settings travel with it in data. Windows WebView2 Runtime must be installed.
Personal packages contain private credentials: keep the entire folder private.
Public packages start empty; supply your own twitch-secrets.json beside the exe.
"@ | Set-Content -LiteralPath (Join-Path $dest 'START HERE.txt') -Encoding UTF8
    Write-Host "Portable app: $dest"
} finally { Pop-Location }
