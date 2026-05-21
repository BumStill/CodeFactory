# Generate the CodeFactory app icon source PNG (1024x1024).
#
# Design: "Crystallization" — a single bold asymmetric gem silhouette on
# the Shandong-Taishan orange-to-crimson gradient.
#
# Concept: CodeFactory's real product isn't code — it's the capture and
# crystallisation of human inspiration into shipped form. The icon is a
# raw idea (formless) shaped by the factory process into a faceted gem
# (geometric, precious, permanent). One white shape. No competing
# elements. Reads as a precious gem at any size, from 16px to 1024px.
#
# Run from project root:
#   .\scripts\generate-icon.ps1
#   pnpm tauri icon icon-source.png

Add-Type -AssemblyName System.Drawing

$W = 1024
$radius = [int]($W * 0.20)

$bmp = New-Object System.Drawing.Bitmap($W, $W)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode     = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
$g.PixelOffsetMode   = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
$g.Clear([System.Drawing.Color]::Transparent)

# ── Background: rounded square with the forge-fire gradient ─────────────────
function New-RoundRectPath {
    param([float]$X, [float]$Y, [float]$W, [float]$H, [float]$R)
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = $R * 2
    $path.AddArc($X,           $Y,           $d, $d, 180, 90)
    $path.AddArc($X + $W - $d, $Y,           $d, $d, 270, 90)
    $path.AddArc($X + $W - $d, $Y + $H - $d, $d, $d, 0,   90)
    $path.AddArc($X,           $Y + $H - $d, $d, $d, 90,  90)
    $path.CloseFigure()
    return $path
}

$bgPath = New-RoundRectPath 0 0 $W $W $radius

# Diagonal: orange (top-left, hot) → crimson (bottom-right, cooled).
$brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
    (New-Object System.Drawing.Point(0, 0)),
    (New-Object System.Drawing.Point($W, $W)),
    [System.Drawing.Color]::FromArgb(255, 255, 107, 26),
    [System.Drawing.Color]::FromArgb(255, 199, 22,  28))
$g.FillPath($brush, $bgPath)
$brush.Dispose()

# Subtle top-left highlight for depth.
$half = [int]($W / 2)
$highlight = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
    (New-Object System.Drawing.Point(0, 0)),
    (New-Object System.Drawing.Point($half, $half)),
    [System.Drawing.Color]::FromArgb(36, 255, 255, 255),
    [System.Drawing.Color]::FromArgb(0,  255, 255, 255))
$g.FillPath($highlight, $bgPath)
$highlight.Dispose()

# ── The Gem ─────────────────────────────────────────────────────────────────
# Asymmetric rhombus with an internal facet line dividing it into a bright
# upper face and a slightly shaded lower face. Tilted ~7° for dynamism.

$cx = $W / 2.0
$cy = $W / 2.0 + ($W * 0.02)

$gemHalfW = $W * 0.215
$gemTop   = $cy - $W * 0.32
$gemBot   = $cy + $W * 0.30
$gemMidY  = $cy - $W * 0.02

$topTipDx = -$W * 0.015
$botTipDx =  $W * 0.020

$tipTop    = New-Object System.Drawing.PointF([float]($cx + $topTipDx), [float]$gemTop)
$shoulderR = New-Object System.Drawing.PointF([float]($cx + $gemHalfW), [float]$gemMidY)
$tipBot    = New-Object System.Drawing.PointF([float]($cx + $botTipDx), [float]$gemBot)
$shoulderL = New-Object System.Drawing.PointF([float]($cx - $gemHalfW), [float]$gemMidY)

function Rotate-Point {
    param($P, [float]$Cx, [float]$Cy, [float]$Deg)
    $rad = $Deg * [Math]::PI / 180.0
    $cs = [Math]::Cos($rad)
    $sn = [Math]::Sin($rad)
    $dx = $P.X - $Cx
    $dy = $P.Y - $Cy
    return New-Object System.Drawing.PointF(
        [float]($Cx + $dx * $cs - $dy * $sn),
        [float]($Cy + $dx * $sn + $dy * $cs))
}
$tilt = 7
$tipTop    = Rotate-Point $tipTop    $cx $cy $tilt
$shoulderR = Rotate-Point $shoulderR $cx $cy $tilt
$tipBot    = Rotate-Point $tipBot    $cx $cy $tilt
$shoulderL = Rotate-Point $shoulderL $cx $cy $tilt

$brightWhite = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::White)
$upperFacet = New-Object System.Drawing.Drawing2D.GraphicsPath
$upperFacet.AddPolygon(@($tipTop, $shoulderR, $shoulderL))
$g.FillPath($brightWhite, $upperFacet)

$shadedWhite = New-Object System.Drawing.SolidBrush(
    [System.Drawing.Color]::FromArgb(235, 255, 255, 255))
$lowerFacet = New-Object System.Drawing.Drawing2D.GraphicsPath
$lowerFacet.AddPolygon(@($shoulderR, $tipBot, $shoulderL))
$g.FillPath($shadedWhite, $lowerFacet)

$brightWhite.Dispose()
$shadedWhite.Dispose()

# ── Save ────────────────────────────────────────────────────────────────────
$outPath = Join-Path (Get-Location) "icon-source.png"
$bmp.Save($outPath, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose()
$bmp.Dispose()

Write-Host "Wrote $outPath ($W x $W PNG)"
Write-Host "Next: pnpm tauri icon icon-source.png"
