# Reproducible vector drawing for the multi-resolution Windows icon.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$images = @()
foreach ($size in @(16, 24, 32, 48, 64, 128, 256)) {
    $bitmap = New-Object System.Drawing.Bitmap($size, $size)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $graphics.ScaleTransform($size / 256.0, $size / 256.0)
        $dark = New-Object System.Drawing.SolidBrush([System.Drawing.ColorTranslator]::FromHtml('#13283f'))
        $mint = New-Object System.Drawing.Pen([System.Drawing.ColorTranslator]::FromHtml('#35d7c0'), 24)
        $white = [System.Drawing.Brushes]::White
        $round = New-Object System.Drawing.Drawing2D.GraphicsPath
        $round.AddArc(8,8,104,104,180,90); $round.AddArc(144,8,104,104,270,90)
        $round.AddArc(144,144,104,104,0,90); $round.AddArc(8,144,104,104,90,90); $round.CloseFigure()
        $graphics.FillPath($dark,$round)
        $mint.StartCap = 'Round'; $mint.EndCap = 'Round'
        $graphics.DrawArc($mint,59,46,138,138,180,180)
        $graphics.DrawLine($mint,59,115,59,161); $graphics.DrawLine($mint,197,115,197,161)
        $graphics.FillRectangle($white,36,137,46,57); $graphics.FillRectangle($white,174,137,46,57)
        foreach($x in @(48,62,186,200)) { $graphics.FillRectangle($dark,$x,149,8,16) }
        $mint.Width = 8
        $graphics.DrawLine($mint,100,129,156,129); $graphics.DrawLine($mint,144,117,156,129); $graphics.DrawLine($mint,144,141,156,129)
        $graphics.DrawLine($mint,100,163,156,163); $graphics.DrawLine($mint,112,151,100,163); $graphics.DrawLine($mint,112,175,100,163)
        $stream = New-Object System.IO.MemoryStream
        $bitmap.Save($stream,[System.Drawing.Imaging.ImageFormat]::Png)
        $images += [PSCustomObject]@{Size=$size; Data=$stream.ToArray()}
        if($size-eq 256) { $bitmap.Save((Join-Path $PSScriptRoot 'assets\usbbridge.png'),[System.Drawing.Imaging.ImageFormat]::Png) }
        $stream.Dispose(); $mint.Dispose(); $dark.Dispose(); $round.Dispose()
    } finally { $graphics.Dispose(); $bitmap.Dispose() }
}
$file = [System.IO.File]::Create((Join-Path $PSScriptRoot 'assets\usbbridge.ico'))
$writer = New-Object System.IO.BinaryWriter($file)
try {
    $writer.Write([uint16]0); $writer.Write([uint16]1); $writer.Write([uint16]$images.Count)
    $offset = 6 + 16 * $images.Count
    foreach($entry in $images) {
        $dimension = if($entry.Size-eq 256){0}else{$entry.Size}
        $writer.Write([byte]$dimension); $writer.Write([byte]$dimension); $writer.Write([uint16]0)
        $writer.Write([uint16]1); $writer.Write([uint16]32); $writer.Write([uint32]$entry.Data.Length); $writer.Write([uint32]$offset)
        $offset += $entry.Data.Length
    }
    foreach($entry in $images) { $writer.Write([byte[]]$entry.Data) }
} finally { $writer.Dispose(); $file.Dispose() }
