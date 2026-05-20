# Generate the CodeFactory app icon source PNG (1024x1024) using System.Drawing.
# Run from project root:  .\scripts\generate-icon.ps1
# Then:                   pnpm tauri icon icon-source.png
#
# Design:
#   - Indigo→fuchsia diagonal gradient on rounded square (distinct from
#     PowerShell-blue / Codex-blue palettes)
#   - "CF" compound monogram: a thick C ring with an F nested inside
#   - Tiny 4-point spark in the upper-right corner of the F = "AI"
#   - Subtle top-left highlight for depth

Add-Type -AssemblyName System.Drawing

$W = 1024
$radius = [int]($W * 0.20)

$bmp = New-Object System.Drawing.Bitmap($W, $W)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode     = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
$g.PixelOffsetMode   = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
$g.Clear([System.Drawing.Color]::Transparent)

# ── Rounded-rectangle path ───────────────────────────────────────────────────
function New-RoundRectPath {
    param([float]$X, [float]$Y, [float]$W, [float]$H, [float]$R)
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = $R * 2
    $path.AddArc($X,         $Y,         $d, $d, 180, 90)
    $path.AddArc($X + $W-$d, $Y,         $d, $d, 270, 90)
    $path.AddArc($X + $W-$d, $Y + $H-$d, $d, $d, 0,   90)
    $path.AddArc($X,         $Y + $H-$d, $d, $d, 90,  90)
    $path.CloseFigure()
    return $path
}

$bgPath = New-RoundRectPath 0 0 $W $W $radius

# Shandong Taishan jersey palette: vivid orange (#FF6B1A) → deep crimson (#D71E1E)
# Warm, energetic, instantly distinct from any developer-tool blue.
$brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
    (New-Object System.Drawing.Point(0, 0)),
    (New-Object System.Drawing.Point($W, $W)),
    [System.Drawing.Color]::FromArgb(255, 255, 107, 26),
    [System.Drawing.Color]::FromArgb(255, 215, 30,  30))
$g.FillPath($brush, $bgPath)
$brush.Dispose()

# Subtle inner highlight from top-left for depth
$half = [int]($W / 2)
$highlight = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
    (New-Object System.Drawing.Point(0, 0)),
    (New-Object System.Drawing.Point($half, $half)),
    [System.Drawing.Color]::FromArgb(50, 255, 255, 255),
    [System.Drawing.Color]::FromArgb(0,  255, 255, 255))
$g.FillPath($highlight, $bgPath)
$highlight.Dispose()

# ── "CF" monogram ────────────────────────────────────────────────────────────
# Layout: a wide C on the left, with an F nested inside its open side.
# Both shapes drawn as thick white strokes; the F sits inside the C's mouth
# so they read as one compound mark.

$white = [System.Drawing.Color]::White
$strokeWidth = 95

# Common geometry
$cx = $W / 2
$cy = $W / 2
$cRadius = 260                # C outer radius
$cInnerR = $cRadius - $strokeWidth

# ── C: a thick arc, opening to the right (angles 35° to 325° CCW) ────────────
# DrawArc draws CW from start, so start at 35° (measuring from 3-o'clock CW)
# Actually System.Drawing uses CW with 0° at 3-o'clock, so to draw an open-right
# arc spanning ~290° we start at 35° and sweep -290° (CCW) — easier to draw two
# half arcs.

$arcPen = New-Object System.Drawing.Pen($white, $strokeWidth)
$arcPen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
$arcPen.EndCap   = [System.Drawing.Drawing2D.LineCap]::Round

$arcBox = New-Object System.Drawing.RectangleF(
    [float]($cx - $cRadius - 100),
    [float]($cy - $cRadius),
    [float]($cRadius * 2),
    [float]($cRadius * 2))

# Sweep from 40° down through 360° to 320° (= 280° sweep, opening at right)
$g.DrawArc($arcPen, $arcBox, 40, 280)

# ── F nested inside the C opening ────────────────────────────────────────────
# Vertical stem
$fStemX = [int]($cx + 70)
$fStemTop = [int]($cy - $cRadius + 30)
$fStemBottom = [int]($cy + $cRadius - 30)
$fStemWidth = $strokeWidth

$g.FillRectangle(
    (New-Object System.Drawing.SolidBrush($white)),
    $fStemX, $fStemTop, $fStemWidth, ($fStemBottom - $fStemTop))

# Top horizontal bar of F
$fTopWidth = 280
$g.FillRectangle(
    (New-Object System.Drawing.SolidBrush($white)),
    $fStemX, $fStemTop, $fTopWidth, $strokeWidth)

# Middle horizontal bar of F (slightly shorter)
$fMidWidth = 200
$fMidY = [int]($cy - 30)
$g.FillRectangle(
    (New-Object System.Drawing.SolidBrush($white)),
    $fStemX, $fMidY, $fMidWidth, ($strokeWidth - 15))

# Round the cap-ends of the bars by drawing circles on the rightmost edges
$capR = [int]($strokeWidth / 2)
$capR2 = [int](($strokeWidth - 15) / 2)
$g.FillEllipse(
    (New-Object System.Drawing.SolidBrush($white)),
    ($fStemX + $fTopWidth - $strokeWidth), $fStemTop, $strokeWidth, $strokeWidth)
$g.FillEllipse(
    (New-Object System.Drawing.SolidBrush($white)),
    ($fStemX + $fMidWidth - ($strokeWidth - 15)), $fMidY, ($strokeWidth - 15), ($strokeWidth - 15))
$g.FillEllipse(
    (New-Object System.Drawing.SolidBrush($white)),
    $fStemX, ($fStemBottom - $strokeWidth), $strokeWidth, $strokeWidth)
$g.FillEllipse(
    (New-Object System.Drawing.SolidBrush($white)),
    $fStemX, $fStemTop, $strokeWidth, $strokeWidth)

# ── AI "spark" — 4-point star top-right ──────────────────────────────────────
function Add-Spark {
    param([float]$Cx, [float]$Cy, [float]$Size)
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $outer = $Size
    $inner = $Size * 0.25
    $pts = @(
        (New-Object System.Drawing.PointF([float]$Cx,         [float]($Cy - $outer))),
        (New-Object System.Drawing.PointF([float]($Cx + $inner), [float]($Cy - $inner))),
        (New-Object System.Drawing.PointF([float]($Cx + $outer), [float]$Cy)),
        (New-Object System.Drawing.PointF([float]($Cx + $inner), [float]($Cy + $inner))),
        (New-Object System.Drawing.PointF([float]$Cx,         [float]($Cy + $outer))),
        (New-Object System.Drawing.PointF([float]($Cx - $inner), [float]($Cy + $inner))),
        (New-Object System.Drawing.PointF([float]($Cx - $outer), [float]$Cy)),
        (New-Object System.Drawing.PointF([float]($Cx - $inner), [float]($Cy - $inner)))
    )
    $path.AddPolygon($pts)
    return $path
}

$spark = Add-Spark ($fStemX + $fTopWidth + 70) ($fStemTop + 40) 55
$g.FillPath((New-Object System.Drawing.SolidBrush($white)), $spark)

$arcPen.Dispose()

# Save
$outPath = Join-Path (Get-Location) "icon-source.png"
$bmp.Save($outPath, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose()
$bmp.Dispose()

Write-Host "Wrote $outPath ($W x $W PNG)"
Write-Host "Next: pnpm tauri icon icon-source.png"
