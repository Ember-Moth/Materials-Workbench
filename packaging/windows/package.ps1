# Packages the Windows release (docs/design/release.md §2.1, §4):
#
#   Materials-Workbench-<version>-windows-x64-setup.exe   Inno Setup installer
#   Materials-Workbench-<version>-windows-x64.zip         portable
#   mw-<version>-windows-x64.zip                          the command line alone
#
#   pwsh packaging/windows/package.ps1 [-Out OUT_DIR]     default: target\packages
#
# Needs the MSVC toolchain, Inno Setup 6 (choco install innosetup) and
# cargo-about, as the macOS and Linux scripts (packaging/common.sh); with
# $env:MW_NO_LICENSES = "1" it writes a stub instead of the licenses, for
# trying the script only. Not signed (§4, §10).
param([string]$Out = "target\packages")

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

# Runs a program and stops when it fails, which PowerShell does not do for
# programs on its own.
function Invoke-Native {
    $command, $rest = $args
    & $command @rest
    if ($LASTEXITCODE -ne 0) { throw "$command failed with exit code $LASTEXITCODE" }
}

$Root = (Resolve-Path "$PSScriptRoot\..\..").Path
Set-Location $Root
New-Item -ItemType Directory -Force $Out | Out-Null
$Out = (Resolve-Path $Out).Path

# One version for the tag, the application and the packages (§2.2).
$Version = ((cargo pkgid -p workbench) -split '[#@]')[-1]
$Target = "x86_64-pc-windows-msvc"
$Name = "windows-x64"
Write-Host "Materials Workbench $Version, $Name"

Invoke-Native cargo build --release --locked -p workbench -p mw-cli --bins --target $Target
$Bin = "target\$Target\release"

$Stage = "$Root\target\packaging\windows"
if (Test-Path $Stage) { Remove-Item -Recurse -Force $Stage }
$App = "$Stage\Materials-Workbench-$Version-$Name"
New-Item -ItemType Directory -Force $App | Out-Null

$Licenses = "$App\THIRD-PARTY-LICENSES.html"
if ($env:MW_NO_LICENSES -eq "1") {
    Write-Warning "MW_NO_LICENSES=1, the package lacks the third-party licenses"
    Set-Content -Encoding utf8 $Licenses "<!doctype html><title>Third-party licenses</title><p>Not generated (MW_NO_LICENSES=1).</p>"
} else {
    if (-not (Get-Command cargo-about -ErrorAction SilentlyContinue)) {
        throw "cargo-about is needed for the third-party licenses: cargo install cargo-about --locked"
    }
    Invoke-Native cargo about generate --locked --fail --config about.toml --target $Target --output-file $Licenses about.hbs
}
Copy-Item "$Bin\materials-workbench.exe", "$Bin\mw.exe", LICENSE-MIT, LICENSE-APACHE $App

# Signing (not set up): the two executables are signed here, before they go
# into the archive and the installer.

Compress-Archive -Force -Path $App -DestinationPath "$Out\Materials-Workbench-$Version-$Name.zip"

$Iscc = @(
    (Get-Command iscc -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source),
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $Iscc) { throw "Inno Setup 6 is needed: choco install innosetup" }
Invoke-Native $Iscc /Qp "/DVersion=$Version" "/DStage=$App" "/DOutput=$Out" packaging\windows\materials-workbench.iss

$Cli = "$Stage\mw-$Version-$Name"
New-Item -ItemType Directory -Force $Cli | Out-Null
Copy-Item "$Bin\mw.exe", LICENSE-MIT, LICENSE-APACHE, $Licenses $Cli
Compress-Archive -Force -Path $Cli -DestinationPath "$Out\mw-$Version-$Name.zip"

Get-ChildItem $Out -Filter "*$Version-$Name*" | ForEach-Object { Write-Host $_.FullName }
