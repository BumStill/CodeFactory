# Usage:
#   .\scripts\bump-version.ps1 0.3.1        # explicit version
#   .\scripts\bump-version.ps1              # auto-bump patch (0.3.0 -> 0.3.1)
param(
    [string]$Version = ""
)

Set-Location (Split-Path $PSScriptRoot -Parent)

# ── Resolve version ──────────────────────────────────────────────────────────
if (-not $Version) {
    $lastTag = git describe --tags --abbrev=0 2>$null
    if (-not $lastTag) { $lastTag = "v0.0.0" }
    $parts = $lastTag.TrimStart("v").Split(".")
    $Version = "$($parts[0]).$($parts[1]).$([int]$parts[2] + 1)"
    Write-Host "Auto-bump: $lastTag -> v$Version"
}

if ($Version -notmatch '^\d+\.\d+\.\d+$') {
    Write-Error "Version must be semver (e.g. 1.2.3)"
    exit 1
}

$tag = "v$Version"

# Guard: tag must not already exist
if (git tag -l $tag) {
    Write-Error "Tag $tag already exists"
    exit 1
}

# Guard: working tree must be clean
$dirty = git status --porcelain
if ($dirty) {
    Write-Error "Working tree is dirty. Commit or stash changes first."
    exit 1
}

# ── Update package.json ───────────────────────────────────────────────────────
$pkgPath = "package.json"
$pkg = Get-Content $pkgPath -Raw | ConvertFrom-Json
$pkg.version = $Version
$pkg | ConvertTo-Json -Depth 10 | Set-Content $pkgPath -Encoding utf8NoBOM
Write-Host "  package.json          -> $Version"

# ── Update tauri.conf.json ────────────────────────────────────────────────────
$tauriPath = "src-tauri/tauri.conf.json"
$tauri = Get-Content $tauriPath -Raw | ConvertFrom-Json
$tauri.version = $Version
$tauri | ConvertTo-Json -Depth 10 | Set-Content $tauriPath -Encoding utf8NoBOM
Write-Host "  tauri.conf.json       -> $Version"

# ── Update Cargo.toml (first version = "..." line) ───────────────────────────
$cargoPath = "src-tauri/Cargo.toml"
$lines = Get-Content $cargoPath
$hit = $false
$lines = $lines | ForEach-Object {
    if (-not $hit -and $_ -match '^version = "') {
        $hit = $true
        "version = `"$Version`""
    } else { $_ }
}
$lines | Set-Content $cargoPath -Encoding utf8NoBOM
Write-Host "  Cargo.toml            -> $Version"

# ── Commit + tag + push ───────────────────────────────────────────────────────
git add package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "chore: bump version to $Version"
git tag $tag
Write-Host ""
Write-Host "Pushing $tag to origin..."
git push origin main --tags

Write-Host ""
Write-Host "Done! GitHub Actions will build and publish the release automatically."
Write-Host "Track progress: https://github.com/BumStill/CodeFactory/actions"
