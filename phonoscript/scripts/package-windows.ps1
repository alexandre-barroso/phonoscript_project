$ErrorActionPreference = "Stop"

if ($env:OS -ne "Windows_NT") {
    throw "package-windows.ps1 must run on Windows."
}

function Assert-PhonoScriptRun {
    param(
        [Parameter(Mandatory = $true)][string]$Interpreter,
        [Parameter(Mandatory = $true)][string]$Script
    )
    & $Interpreter --quiet $Script
    if ($LASTEXITCODE -ne 0) {
        throw "PhonoScript failed for $Script with exit code $LASTEXITCODE."
    }
}

function Assert-NoMachinePath {
    param([Parameter(Mandatory = $true)][string]$Binary)
    $content = [Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($Binary))
    if ($content -match '(?i)[A-Z]:\\Users\\|/Users/|/Volumes/|/home/runner/') {
        throw "Machine-specific path found in $Binary."
    }
}

function Assert-PeSubsystem {
    param(
        [Parameter(Mandatory = $true)][string]$Binary,
        [Parameter(Mandatory = $true)][UInt16]$Expected,
        [Parameter(Mandatory = $true)][string]$Description
    )
    [byte[]]$bytes = [IO.File]::ReadAllBytes($Binary)
    if ($bytes.Length -lt 256 -or $bytes[0] -ne 0x4D -or $bytes[1] -ne 0x5A) {
        throw "$Binary is not a valid PE executable."
    }
    $peOffset = [BitConverter]::ToInt32($bytes, 0x3C)
    $subsystemOffset = $peOffset + 24 + 68
    if ($peOffset -lt 0 -or $subsystemOffset + 2 -gt $bytes.Length) {
        throw "$Binary has an invalid PE optional header."
    }
    $actual = [BitConverter]::ToUInt16($bytes, $subsystemOffset)
    if ($actual -ne $Expected) {
        throw "$Binary has PE subsystem $actual; expected $Expected ($Description)."
    }
}

function Assert-NoDynamicCrt {
    param([Parameter(Mandatory = $true)][string]$Binary)
    $content = [Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($Binary))
    if ($content -match '(?i)VCRUNTIME[0-9]*\.dll|api-ms-win-crt-') {
        throw "Dynamic Visual C++ runtime dependency found in $Binary."
    }
}

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$phonoscriptRoot = Split-Path -Parent $scriptRoot
$workspaceRoot = Split-Path -Parent $phonoscriptRoot
$docsRoot = Join-Path $workspaceRoot "docs"
$compiledRoot = if ($env:PHONOSCRIPT_COMPILED_DIR) {
    $env:PHONOSCRIPT_COMPILED_DIR
} else {
    Join-Path $phonoscriptRoot "compiled"
}
$platformRoot = Join-Path $compiledRoot "windows"
$nativeArchitecture = if ($env:PROCESSOR_ARCHITEW6432) {
    $env:PROCESSOR_ARCHITEW6432
} else {
    $env:PROCESSOR_ARCHITECTURE
}
$architecture = switch ($nativeArchitecture.ToUpperInvariant()) {
    "ARM64" { "aarch64" }
    "AMD64" { "x86_64" }
    default { throw "Unsupported Windows architecture: $nativeArchitecture" }
}
$packageName = "PhonoScript-windows-$architecture"
$packageRoot = Join-Path $platformRoot $packageName
$archivePath = Join-Path $platformRoot "$packageName.zip"
$targetRoot = if ($env:CARGO_TARGET_DIR) {
    if ([IO.Path]::IsPathRooted($env:CARGO_TARGET_DIR)) {
        $env:CARGO_TARGET_DIR
    } else {
        Join-Path $workspaceRoot $env:CARGO_TARGET_DIR
    }
} else {
    Join-Path $workspaceRoot "target"
}

$cargoHome = if ($env:CARGO_HOME) {
    $env:CARGO_HOME
} else {
    Join-Path $env:USERPROFILE ".cargo"
}
$remapFlags = @(
    "--remap-path-prefix=$workspaceRoot=."
    "--remap-path-prefix=$cargoHome=.cargo"
    "-Ctarget-feature=+crt-static"
) -join " "
$env:RUSTFLAGS = (($env:RUSTFLAGS, $remapFlags) -join " ").Trim()

Push-Location $workspaceRoot
try {
    cargo build --release --locked -p phonoscript --bin phonoscript
    if ($LASTEXITCODE -ne 0) {
        throw "The Windows PhonoScript release build failed with exit code $LASTEXITCODE."
    }
} finally {
    Pop-Location
}

$binary = Join-Path $targetRoot "release\phonoscript.exe"
if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
    throw "Release binary is missing: $binary"
}
Assert-NoMachinePath -Binary $binary
Assert-NoDynamicCrt -Binary $binary
Assert-PeSubsystem `
    -Binary $binary -Expected 3 -Description "Windows console"

if (Test-Path $platformRoot) {
    Remove-Item -Recurse -Force $platformRoot
}
New-Item -ItemType Directory -Force `
    -Path $packageRoot, `
    (Join-Path $packageRoot "docs"), `
    (Join-Path $packageRoot "validation\analyses"), `
    (Join-Path $packageRoot "fixtures") | Out-Null

Copy-Item $binary (Join-Path $packageRoot "phonoscript.exe")
Copy-Item (Join-Path $phonoscriptRoot "LICENSE") $packageRoot
Copy-Item (Join-Path $docsRoot "PhonoScript-Language-Manual.pdf") `
    (Join-Path $packageRoot "docs\PhonoScript-Language-Manual.pdf")
Copy-Item -Recurse -Force (Join-Path $phonoscriptRoot "validation\analyses\*") `
    (Join-Path $packageRoot "validation\analyses")
Copy-Item -Force (Join-Path $phonoscriptRoot "fixtures\reference\*.ottab") `
    (Join-Path $packageRoot "fixtures")
Copy-Item (Join-Path $scriptRoot "install-user.ps1") `
    (Join-Path $packageRoot "install-user.ps1")

$packageInterpreter = Join-Path $packageRoot "phonoscript.exe"
& $packageInterpreter --version | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "The packaged PhonoScript interpreter did not start successfully."
}
Get-ChildItem -Recurse -File (Join-Path $packageRoot "validation\analyses") `
    -Filter "*.phont" | Sort-Object FullName | ForEach-Object {
        Assert-PhonoScriptRun -Interpreter $packageInterpreter -Script $_.FullName
    }

$smokeRoot = Join-Path $platformRoot "package-smoke"
try {
    New-Item -ItemType Directory -Force -Path $smokeRoot | Out-Null
    Get-ChildItem -File (Join-Path $packageRoot "fixtures\*.ottab") | `
        Sort-Object FullName | ForEach-Object {
            $emitted = Join-Path $smokeRoot ($_.BaseName + ".phont")
            & $packageInterpreter --emit $_.FullName --write $emitted --quiet
            if ($LASTEXITCODE -ne 0) {
                throw "Could not emit $($_.FullName)."
            }
            Assert-PhonoScriptRun -Interpreter $packageInterpreter -Script $emitted
        }

    Compress-Archive -LiteralPath $packageRoot -DestinationPath $archivePath
    $extractRoot = Join-Path $smokeRoot "archive"
    Expand-Archive -LiteralPath $archivePath -DestinationPath $extractRoot
    $extracted = Join-Path $extractRoot $packageName
    $sample = Get-ChildItem -Recurse -File `
        (Join-Path $extracted "validation\analyses") -Filter "*.phont" | `
        Sort-Object FullName | Select-Object -First 1
    Assert-PhonoScriptRun `
        -Interpreter (Join-Path $extracted "phonoscript.exe") `
        -Script $sample.FullName
    if (-not (Test-Path (Join-Path $extracted "docs\PhonoScript-Language-Manual.pdf"))) {
        throw "The packaged PhonoScript manual is missing."
    }

    $installRoot = Join-Path $smokeRoot "installed"
    & (Join-Path $extracted "install-user.ps1") -InstallRoot $installRoot
    if ($LASTEXITCODE -ne 0) {
        throw "The PhonoScript current-user installer failed."
    }
    & (Join-Path $installRoot "phonoscript.exe") --version | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "The installed PhonoScript interpreter did not start."
    }
} finally {
    if (Test-Path $smokeRoot) {
        Remove-Item -Recurse -Force $smokeRoot
    }
}

Write-Output $archivePath
