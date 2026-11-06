# Release build: embeds the Twitch API credentials at compile time so the
# shipped app needs no re-registration (same key as the original app).
# Reads .twitch-secrets.json (gitignored) into env vars for this process only.
# Never prints credential values.
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

$secretsPath = Join-Path (Get-Location) ".twitch-secrets.json"
if (Test-Path $secretsPath) {
    $secrets = Get-Content $secretsPath -Raw | ConvertFrom-Json
    if ($secrets.clientId -and $secrets.clientSecret) {
        $env:TWITCH_CLIENT_ID = $secrets.clientId
        $env:TWITCH_CLIENT_SECRET = $secrets.clientSecret
        Write-Host "Credentials: loaded for embedding."
    } else {
        Write-Warning "Secrets file found but incomplete - building WITHOUT embedded credentials."
    }
} else {
    Write-Warning "No .twitch-secrets.json found - building WITHOUT embedded credentials (env/file fallback still works at runtime)."
}

npm run build
if ($LASTEXITCODE -ne 0) { throw "frontend build failed" }
# Tauri CLI logs progress on stderr; don't let ErrorActionPreference=Stop
# treat that as fatal in Windows PowerShell 5.1.
$prevEap = $ErrorActionPreference
$ErrorActionPreference = "Continue"
npx tauri build
$tauriExit = $LASTEXITCODE
$ErrorActionPreference = $prevEap
if ($tauriExit -ne 0) { throw "tauri build failed" }
Write-Host "Release build complete: src-tauri/target/release/bundle/"
