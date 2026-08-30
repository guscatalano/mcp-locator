<#
.SYNOPSIS
  Builds the mcp-locator MSI.

.DESCRIPTION
  Compiles the Rust binaries in release mode, bundles the gateway into a single .mjs, stages
  everything into one flat directory, and hands that to WiX.

  Staging is a separate step on purpose: the .wxs then refers only to files inside the stage,
  so what ends up in the MSI is exactly what this script put there, and a stale artifact from
  a previous layout cannot be picked up by a wildcard.

.PARAMETER Sign
  Path to a code-signing certificate (.pfx). Without it the MSI and both executables are
  unsigned, which means SmartScreen will warn on any machine other than this one.
#>
[CmdletBinding()]
param(
    [string]$Version = '0.1.0',
    [string]$Sign,
    [securestring]$SignPassword
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$stage = Join-Path $PSScriptRoot 'stage'
$out = Join-Path $PSScriptRoot 'dist'

Write-Host '==> Rust binaries (release)' -ForegroundColor Cyan
& cargo build --release --manifest-path (Join-Path $repo 'broker\Cargo.toml')
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

Write-Host '==> TypeScript build and gateway bundle' -ForegroundColor Cyan
Push-Location $repo
try {
    & npm run build
    if ($LASTEXITCODE -ne 0) { throw 'npm run build failed' }
    & npm run bundle --workspace '@mcp-locator/gateway'
    if ($LASTEXITCODE -ne 0) { throw 'gateway bundle failed' }
}
finally { Pop-Location }

Write-Host '==> Staging' -ForegroundColor Cyan
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Path $stage | Out-Null
New-Item -ItemType Directory -Path (Join-Path $stage 'gateway') | Out-Null

$release = Join-Path $repo 'broker\target\release'
Copy-Item (Join-Path $release 'mcp-locator-broker.exe') $stage
Copy-Item (Join-Path $release 'mcp-locator-consent.exe') $stage
Copy-Item (Join-Path $repo 'LICENSE') $stage
Copy-Item (Join-Path $repo 'packages\gateway\dist\bundle\*') (Join-Path $stage 'gateway') -Recurse

# The card is generated rather than copied so its version can never drift from the binary it
# points at — a mismatch there would show up as a broker that reports one version and behaves
# like another.
$card = [ordered]@{
    '$schema'   = 'https://mcp-locator.dev/schemas/v1/local-server-card.schema.json'
    name        = 'io.mcplocator.broker'
    version     = $Version
    title       = 'mcp-locator Broker'
    description = 'The mcp-locator broker daemon. Valid only in the system tier; client libraries refuse to launch it from anywhere but the install root (spec/003 section 3).'
    local       = [ordered]@{
        launch   = [ordered]@{
            type    = 'executable'
            command = '%ProgramFiles%\mcp-locator\mcp-locator-broker.exe'
            args    = @('serve')
        }
        endpoint = [ordered]@{
            type    = 'pipe'
            address = '\\.\pipe\mcp-locator\broker\v1'
        }
        lifetime = [ordered]@{
            # The broker is the thing that runs everything else; idling it out would just mean
            # paying the bootstrap cost again on the next request.
            idleTimeoutSeconds = 0
            shutdown           = 'graceful'
        }
    }
}
$cardPath = Join-Path $stage 'io.mcplocator.broker.card.json'
# WriteAllText rather than Set-Content: Windows PowerShell's utf8 encoding emits a BOM, and a
# card is read by parsers in several languages that have no reason to expect one.
[IO.File]::WriteAllText($cardPath, ($card | ConvertTo-Json -Depth 6))

if ($Sign) {
    Write-Host '==> Signing binaries' -ForegroundColor Cyan
    $signtool = Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\bin\*\x64\signtool.exe' |
        Sort-Object FullName -Descending | Select-Object -First 1
    if (-not $signtool) { throw 'signtool.exe not found; install the Windows SDK' }
    $password = if ($SignPassword) {
        [Runtime.InteropServices.Marshal]::PtrToStringAuto(
            [Runtime.InteropServices.Marshal]::SecureStringToBSTR($SignPassword))
    } else { $null }
    $targets = @(
        (Join-Path $stage 'mcp-locator-broker.exe'),
        (Join-Path $stage 'mcp-locator-consent.exe')
    )
    foreach ($target in $targets) {
        & $signtool.FullName sign /fd SHA256 /f $Sign /p $password /tr http://timestamp.digicert.com /td SHA256 $target
        if ($LASTEXITCODE -ne 0) { throw "signing $target failed" }
    }
}

Write-Host '==> WiX' -ForegroundColor Cyan
New-Item -ItemType Directory -Path $out -Force | Out-Null
$msi = Join-Path $out "mcp-locator-$Version-x64.msi"
& wix build -arch x64 `
    -d "StageDir=$stage" `
    -d "Version=$Version" `
    -o $msi `
    (Join-Path $PSScriptRoot 'mcp-locator.wxs')
if ($LASTEXITCODE -ne 0) { throw 'wix build failed' }

if ($Sign) {
    $signtool = Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\bin\*\x64\signtool.exe' |
        Sort-Object FullName -Descending | Select-Object -First 1
    $password = if ($SignPassword) {
        [Runtime.InteropServices.Marshal]::PtrToStringAuto(
            [Runtime.InteropServices.Marshal]::SecureStringToBSTR($SignPassword))
    } else { $null }
    & $signtool.FullName sign /fd SHA256 /f $Sign /p $password /tr http://timestamp.digicert.com /td SHA256 $msi
    if ($LASTEXITCODE -ne 0) { throw 'signing the MSI failed' }
}
else {
    Write-Warning 'Unsigned build. SmartScreen will warn on any machine but this one; pass -Sign <cert.pfx> for a release.'
}

Write-Host "==> $msi" -ForegroundColor Green
