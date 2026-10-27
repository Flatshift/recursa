// Renders background.svg to background.tiff (660x400 at 1x and 2x, joined as a HiDPI TIFF).
// Usage: swift render.swift   (run from this directory)
import AppKit

let svg = NSImage(contentsOfFile: "background.svg")!
let width = 660, height = 400
var pngs: [String] = []
for scale in [1, 2] {
    let rep = NSBitmapImageRep(
        bitmapDataPlanes: nil, pixelsWide: width * scale, pixelsHigh: height * scale,
        bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
        colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    svg.draw(in: NSRect(x: 0, y: 0, width: width * scale, height: height * scale))
    NSGraphicsContext.restoreGraphicsState()
    let path = NSTemporaryDirectory() + "recursa-bg@\(scale)x.png"
    try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: path))
    pngs.append(path)
}
for args in [["-cathidpicheck"] + pngs + ["-out", pngs[0] + ".tiff"],
             ["-lzw", pngs[0] + ".tiff", "-out", "background.tiff"]] {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: "/usr/bin/tiffutil")
    p.arguments = args
    try! p.run()
    p.waitUntilExit()
}
