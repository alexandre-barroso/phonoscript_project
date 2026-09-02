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
$sourceRoot = Split-Path -Parent $scriptRoot
$convalgenRoot = Split-Path -Parent $sourceRoot
$workspaceRoot = Split-Path -Parent $convalgenRoot
$phonoscriptRoot = Join-Path $workspaceRoot "phonoscript"
$docsRoot = Join-Path $workspaceRoot "docs"
$compiledRoot = if ($env:CONVALGEN_COMPILED_DIR) {
    $env:CONVALGEN_COMPILED_DIR
} else {
    Join-Path $convalgenRoot "compiled"
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
$packageName = "PhonoScript-GUI-windows-$architecture"
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
    cargo build --release --locked -p convalgen --bin phonoscript-gui
    if ($LASTEXITCODE -ne 0) {
        throw "The Windows PhonoScript GUI release build failed with exit code $LASTEXITCODE."
    }
    cargo build --release --locked -p phonoscript --bin phonoscript
    if ($LASTEXITCODE -ne 0) {
        throw "The Windows PhonoScript release build failed with exit code $LASTEXITCODE."
    }
} finally {
    Pop-Location
}

$convalgenBinary = Join-Path $targetRoot "release\phonoscript-gui.exe"
$phonoscriptBinary = Join-Path $targetRoot "release\phonoscript.exe"
foreach ($binary in @($convalgenBinary, $phonoscriptBinary)) {
    if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
        throw "Release binary is missing: $binary"
    }
    Assert-NoMachinePath -Binary $binary
    Assert-NoDynamicCrt -Binary $binary
}
Assert-PeSubsystem `
    -Binary $convalgenBinary -Expected 2 -Description "Windows GUI"
Assert-PeSubsystem `
    -Binary $phonoscriptBinary -Expected 3 -Description "Windows console"

if (Test-Path $platformRoot) {
    Remove-Item -Recurse -Force $platformRoot
}
New-Item -ItemType Directory -Force `
    -Path $packageRoot, `
    (Join-Path $packageRoot "Documentation"), `
    (Join-Path $packageRoot "Projects"), `
    (Join-Path $packageRoot "Validation\analyses"), `
    (Join-Path $packageRoot "Validation\fixtures") | Out-Null

Copy-Item $convalgenBinary (Join-Path $packageRoot "phonoscript-gui.exe")
Copy-Item $phonoscriptBinary (Join-Path $packageRoot "phonoscript.exe")
Copy-Item (Join-Path $sourceRoot "assets\icon\windows\PhonoScript-GUI.ico") $packageRoot
Copy-Item (Join-Path $convalgenRoot "LICENSE") `
    (Join-Path $packageRoot "PHONOSCRIPT-GUI-LICENSE")
Copy-Item (Join-Path $phonoscriptRoot "LICENSE") `
    (Join-Path $packageRoot "PHONOSCRIPT-LICENSE")
Copy-Item (Join-Path $docsRoot "PhonoScript-GUI-User-Guide.pdf") `
    (Join-Path $packageRoot "Documentation\PhonoScript-GUI-User-Guide.pdf")
Copy-Item (Join-Path $docsRoot "PhonoScript-Language-Manual.pdf") `
    (Join-Path $packageRoot "Documentation\PhonoScript-Language-Manual.pdf")
Copy-Item (Join-Path $convalgenRoot "projects\dissertation-complete.ottab") `
    (Join-Path $packageRoot "Projects\dissertation-complete.ottab")
Copy-Item -Recurse -Force (Join-Path $phonoscriptRoot "validation\analyses\*") `
    (Join-Path $packageRoot "Validation\analyses")
Copy-Item -Force (Join-Path $phonoscriptRoot "fixtures\reference\*.ottab") `
    (Join-Path $packageRoot "Validation\fixtures")
Copy-Item (Join-Path $sourceRoot "windows\install-user.ps1") `
    (Join-Path $packageRoot "install-user.ps1")

$packageInterpreter = Join-Path $packageRoot "phonoscript.exe"
& $packageInterpreter --version | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "The packaged PhonoScript interpreter did not start successfully."
}
Get-ChildItem -Recurse -File (Join-Path $packageRoot "Validation\analyses") `
    -Filter "*.phont" | Sort-Object FullName | ForEach-Object {
        Assert-PhonoScriptRun -Interpreter $packageInterpreter -Script $_.FullName
    }

$smokeRoot = Join-Path $platformRoot "package-smoke"
try {
    New-Item -ItemType Directory -Force -Path $smokeRoot | Out-Null
    Get-ChildItem -File (Join-Path $packageRoot "Validation\fixtures\*.ottab") | `
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
        (Join-Path $extracted "Validation\analyses") -Filter "*.phont" | `
        Sort-Object FullName | Select-Object -First 1
    Assert-PhonoScriptRun `
        -Interpreter (Join-Path $extracted "phonoscript.exe") `
        -Script $sample.FullName
    if (-not (Test-Path (Join-Path $extracted "Documentation\PhonoScript-GUI-User-Guide.pdf"))) {
        throw "The packaged user guide is missing."
    }
    if (-not (Test-Path (Join-Path $extracted "Documentation\PhonoScript-Language-Manual.pdf"))) {
        throw "The packaged PhonoScript manual is missing."
    }
    if (-not (Test-Path (Join-Path $extracted "Projects\dissertation-complete.ottab"))) {
        throw "The packaged dissertation project is missing."
    }

    $installRoot = Join-Path $smokeRoot "installed"
    & (Join-Path $extracted "install-user.ps1") -InstallRoot $installRoot
    if ($LASTEXITCODE -ne 0) {
        throw "The PhonoScript GUI current-user installer failed."
    }
    & (Join-Path $installRoot "phonoscript.exe") --version | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "The installed PhonoScript interpreter did not start."
    }
    $phontProgId = (Get-Item "HKCU:\Software\Classes\.phont").GetValue("")
    $phontCommand = (Get-Item `
        "HKCU:\Software\Classes\$phontProgId\shell\open\command").GetValue("")
    if ($phontCommand -notmatch "(?i)phonoscript\.exe") {
        throw ".phont is not associated with the PhonoScript interpreter."
    }
} finally {
    if (Test-Path $smokeRoot) {
        Remove-Item -Recurse -Force $smokeRoot
    }
}

Write-Output $archivePath
