# Build Iskra for 64-bit Windows 10 and 11, as the zip that is handed out:
#
#   powershell -ExecutionPolicy Bypass -File scripts\win-app.ps1
#
# or from cmd, scripts\win-app.cmd, which runs this.
#
# Builds the app (release, with Cargo.lock as it is) and makes
# target\win\Iskra-<version>-Windows-x64.zip of it: a folder Iskra with iskra.exe, which runs from
# wherever it is unzipped. The icon and the name under Properties > Details go into the exe by the
# app's build.rs, which needs the Windows resource compiler of the Visual Studio build tools.
#
# The paths of the sources are kept in the binary, for its panic messages: the user's folder
# (where cargo keeps the crates) and this checkout are written as `~` and `.` instead, so that the
# app handed out says nothing of who built it, or where. The exe is checked for them after.
# It is not signed: Windows asks for More info > Run anyway the first time.

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root
$Target = 'x86_64-pc-windows-msvc'

function Run([string]$Program, [string[]]$Arguments) {
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program $($Arguments -join ' ') failed ($LASTEXITCODE)" }
}

$Version = (cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).packages |
    Where-Object { $_.name -eq 'iskra' } | Select-Object -First 1 -ExpandProperty version
if (-not $Version) { throw 'win-app: the version of the iskra package was not found' }

# The folders whose paths must not stay in the binary: the user's own, cargo's if it is kept
# elsewhere, and this checkout. The longest first, so that one inside another is written whole.
$Hidden = @(
    @{ From = $env:USERPROFILE; To = '~' },
    @{ From = $env:CARGO_HOME; To = '~\.cargo' },
    @{ From = $Root; To = '.' }
) | Where-Object { $_.From } | Sort-Object { $_.From.Length } -Descending

# The flags go to rustc one by one, as CARGO_ENCODED_RUSTFLAGS has them, apart by the unit
# separator: a path with a space in it stays one flag.
$Flags = @()
if ($env:RUSTFLAGS) { $Flags += @($env:RUSTFLAGS -split '\s+' | Where-Object { $_ }) }
foreach ($folder in $Hidden) { $Flags += "--remap-path-prefix=$($folder.From)=$($folder.To)" }
$env:CARGO_ENCODED_RUSTFLAGS = $Flags -join [char]0x1f
Remove-Item Env:RUSTFLAGS -ErrorAction SilentlyContinue

Run 'rustup' @('target', 'add', $Target)
Run 'cargo' @('build', '--release', '--locked', '-p', 'iskra', '--target', $Target)

$Built = Join-Path $Root "target\$Target\release\iskra.exe"
$Out = Join-Path $Root 'target\win'
$Folder = Join-Path $Out 'Iskra'
if (Test-Path $Folder) { Remove-Item -Recurse -Force $Folder }
New-Item -ItemType Directory -Force -Path $Folder | Out-Null
Copy-Item $Built (Join-Path $Folder 'iskra.exe')

# Nothing of the folders above may be left in it, as text or as UTF-16, whatever its case.
$Bytes = [System.IO.File]::ReadAllBytes((Join-Path $Folder 'iskra.exe'))
$Text = [System.Text.Encoding]::GetEncoding(28591).GetString($Bytes)
$Wide = [System.Text.Encoding]::Unicode.GetString($Bytes)
foreach ($folder in $Hidden) {
    foreach ($kept in @($Text, $Wide)) {
        if ($kept.IndexOf($folder.From, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
            throw "win-app: the exe still holds $($folder.From)"
        }
    }
}

$Zip = Join-Path $Out "Iskra-$Version-Windows-x64.zip"
if (Test-Path $Zip) { Remove-Item -Force $Zip }
Compress-Archive -Path $Folder -DestinationPath $Zip
Write-Output "built $Zip (version $Version)"
