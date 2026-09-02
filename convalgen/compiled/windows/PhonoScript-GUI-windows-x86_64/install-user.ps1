param(
    [string]$InstallRoot = (Join-Path $env:LOCALAPPDATA "Programs\PhonoScript GUI")
)

$ErrorActionPreference = "Stop"

function Set-RegistryDefault {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Value
    )
    New-Item -Force -Path $Path | Out-Null
    Set-Item -Path $Path -Value $Value
}

function Add-CurrentUserPath {
    param([Parameter(Mandatory = $true)][string]$Directory)

    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $entries = @($userPath -split ";" | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    $normalizedDirectory = $Directory.TrimEnd("\")
    $alreadyPresent = @($entries | Where-Object {
        $_.Trim().TrimEnd("\") -ieq $normalizedDirectory
    }).Count -gt 0
    if (-not $alreadyPresent) {
        [Environment]::SetEnvironmentVariable(
            "Path",
            (($entries + $Directory) -join ";"),
            "User"
        )
    }

    $processEntries = @($env:Path -split ";")
    if (@($processEntries | Where-Object {
        $_.Trim().TrimEnd("\") -ieq $normalizedDirectory
    }).Count -eq 0) {
        $env:Path = "$Directory;$env:Path"
    }
}

$packageRoot = $PSScriptRoot
New-Item -ItemType Directory -Force -Path $InstallRoot | Out-Null
Copy-Item -Force (Join-Path $packageRoot "phonoscript-gui.exe") $InstallRoot
Copy-Item -Force (Join-Path $packageRoot "phonoscript.exe") $InstallRoot
Copy-Item -Force (Join-Path $packageRoot "PhonoScript-GUI.ico") $InstallRoot
Copy-Item -Force (Join-Path $packageRoot "PHONOSCRIPT-GUI-LICENSE") $InstallRoot
Copy-Item -Force (Join-Path $packageRoot "PHONOSCRIPT-LICENSE") $InstallRoot
Copy-Item -Recurse -Force (Join-Path $packageRoot "Documentation") $InstallRoot
Copy-Item -Recurse -Force (Join-Path $packageRoot "Projects") $InstallRoot
Copy-Item -Recurse -Force (Join-Path $packageRoot "Validation") $InstallRoot

$phonoscriptGui = Join-Path $InstallRoot "phonoscript-gui.exe"
$phonoscript = Join-Path $InstallRoot "phonoscript.exe"
$phonoscriptGuiIcon = Join-Path $InstallRoot "PhonoScript-GUI.ico"

# Register both commands for the current user without elevation.
foreach ($application in @("phonoscript-gui.exe", "phonoscript.exe")) {
    $applicationPath = Join-Path $InstallRoot $application
    $appPathsKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\App Paths\$application"
    Set-RegistryDefault -Path $appPathsKey -Value $applicationPath
    Set-ItemProperty -Path $appPathsKey -Name "Path" -Value $InstallRoot
}
Add-CurrentUserPath -Directory $InstallRoot

# PhonoScript GUI analyses open in the graphical application.
Set-RegistryDefault -Path "HKCU:\Software\Classes\PhonoScriptGUI.Analysis" `
    -Value "PhonoScript GUI analysis"
Set-RegistryDefault -Path "HKCU:\Software\Classes\PhonoScriptGUI.Analysis\DefaultIcon" `
    -Value ('"' + $phonoscriptGuiIcon + '",0')
Set-RegistryDefault -Path "HKCU:\Software\Classes\PhonoScriptGUI.Analysis\shell\open\command" `
    -Value ('"' + $phonoscriptGui + '" "%1"')
Set-RegistryDefault -Path "HKCU:\Software\Classes\.ottab" `
    -Value "PhonoScriptGUI.Analysis"

# The normal .phont action executes the script with PhonoScript. PhonoScript
# GUI is also registered as an explicit graphical editor through Open With.
Set-RegistryDefault -Path "HKCU:\Software\Classes\PhonoScript.Script" `
    -Value "PhonoScript script"
Set-RegistryDefault -Path "HKCU:\Software\Classes\PhonoScript.Script\DefaultIcon" `
    -Value ('"' + $phonoscript + '",0')
Set-RegistryDefault -Path "HKCU:\Software\Classes\PhonoScript.Script\shell\open\command" `
    -Value ('"' + $phonoscript + '" "%1" %*')
Set-RegistryDefault -Path "HKCU:\Software\Classes\.phont" `
    -Value "PhonoScript.Script"
New-Item -Force -Path "HKCU:\Software\Classes\.phont\OpenWithProgids" | Out-Null
New-ItemProperty -Force `
    -Path "HKCU:\Software\Classes\.phont\OpenWithProgids" `
    -Name "PhonoScriptGUI.Editor" `
    -PropertyType String `
    -Value "" | Out-Null
Set-RegistryDefault -Path "HKCU:\Software\Classes\PhonoScriptGUI.Editor" `
    -Value "PhonoScript script in PhonoScript GUI"
Set-RegistryDefault -Path "HKCU:\Software\Classes\PhonoScriptGUI.Editor\shell\open\command" `
    -Value ('"' + $phonoscriptGui + '" "%1"')

& $phonoscript --version | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "The installed PhonoScript interpreter did not start successfully."
}
Write-Host "Installed PhonoScript GUI and PhonoScript for the current user at $InstallRoot"
Write-Host "New terminals can invoke: phonoscript SCRIPT.phont"
