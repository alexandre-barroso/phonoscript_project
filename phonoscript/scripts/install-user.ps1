param(
    [string]$InstallRoot = (Join-Path $env:LOCALAPPDATA "Programs\PhonoScript")
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
    $entries = @($userPath -split ";" | Where-Object {
        -not [string]::IsNullOrWhiteSpace($_)
    })
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
Copy-Item -Force (Join-Path $packageRoot "phonoscript.exe") $InstallRoot
Copy-Item -Force (Join-Path $packageRoot "LICENSE") $InstallRoot
Copy-Item -Recurse -Force (Join-Path $packageRoot "docs") $InstallRoot
Copy-Item -Recurse -Force (Join-Path $packageRoot "validation") $InstallRoot
Copy-Item -Recurse -Force (Join-Path $packageRoot "fixtures") $InstallRoot

$phonoscript = Join-Path $InstallRoot "phonoscript.exe"
$appPathsKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\App Paths\phonoscript.exe"
Set-RegistryDefault -Path $appPathsKey -Value $phonoscript
Set-ItemProperty -Path $appPathsKey -Name "Path" -Value $InstallRoot
Add-CurrentUserPath -Directory $InstallRoot

Set-RegistryDefault -Path "HKCU:\Software\Classes\PhonoScript.Script" `
    -Value "PhonoScript script"
Set-RegistryDefault -Path "HKCU:\Software\Classes\PhonoScript.Script\DefaultIcon" `
    -Value ('"' + $phonoscript + '",0')
Set-RegistryDefault -Path "HKCU:\Software\Classes\PhonoScript.Script\shell\open\command" `
    -Value ('"' + $phonoscript + '" "%1" %*')
Set-RegistryDefault -Path "HKCU:\Software\Classes\.phont" `
    -Value "PhonoScript.Script"

& $phonoscript --version | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "The installed PhonoScript interpreter did not start successfully."
}
Write-Host "Installed PhonoScript for the current user at $InstallRoot"
Write-Host "New terminals can invoke: phonoscript SCRIPT.phont"
