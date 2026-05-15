param(
    [Parameter(Mandatory)]
    [string]$Version
)

# Validate semver format
if ($Version -notmatch '^\d+\.\d+\.\d+$') {
    Write-Error "Version must be in semver format (e.g. 1.2.3)"
    exit 1
}

# Update version in src-tauri/tauri.conf.json
$tauriConfPath = "src-tauri/tauri.conf.json"
$tauriConf = Get-Content $tauriConfPath -Raw | ConvertFrom-Json
$tauriConf.version = $Version
$tauriConf | ConvertTo-Json -Depth 10 | Set-Content $tauriConfPath -Encoding utf8
Write-Host "Updated $tauriConfPath -> $Version"

# Update version in src-tauri/Cargo.toml (first occurrence of version = "...")
$cargoPath = "src-tauri/Cargo.toml"
$cargoContent = Get-Content $cargoPath
$updated = $false
$cargoContent = $cargoContent | ForEach-Object {
    if (-not $updated -and $_ -match '^version = ".*"') {
        $updated = $true
        "version = `"$Version`""
    } else {
        $_
    }
}
$cargoContent | Set-Content $cargoPath -Encoding utf8
Write-Host "Updated $cargoPath -> $Version"

# Update version in package.json
$pkgPath = "package.json"
$pkg = Get-Content $pkgPath -Raw | ConvertFrom-Json
$pkg.version = $Version
$pkg | ConvertTo-Json -Depth 10 | Set-Content $pkgPath -Encoding utf8
Write-Host "Updated $pkgPath -> $Version"

Write-Host ""
Write-Host "Version bumped to $Version. Next steps:"
Write-Host "  git add -A"
Write-Host "  git commit -m `"chore: bump version to $Version`""
Write-Host "  git tag v$Version"
Write-Host "  git push origin main --tags"
