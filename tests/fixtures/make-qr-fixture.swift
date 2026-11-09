// 生成二维码测试图 ocr-qr.png：左边一个指向 https://example.com/helmp 的二维码，右边一行「扫码打开 Helmp」。
// 用法：swift tests/fixtures/make-qr-fixture.swift tests/fixtures/ocr-qr.png
import AppKit
import CoreImage

let output = CommandLine.arguments.dropFirst().first ?? "ocr-qr.png"
let filter = CIFilter(name: "CIQRCodeGenerator")!
filter.setValue("https://example.com/helmp".data(using: .utf8), forKey: "inputMessage")
filter.setValue("M", forKey: "inputCorrectionLevel")
let qr = filter.outputImage!.transformed(by: CGAffineTransform(scaleX: 10, y: 10))
let qrRep = NSCIImageRep(ciImage: qr)

let size = NSSize(width: 1100, height: 400)
let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: Int(size.width), pixelsHigh: Int(size.height),
    bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
    colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
NSColor.white.setFill()
NSRect(origin: .zero, size: size).fill()
qrRep.draw(in: NSRect(x: 40, y: 45, width: qr.extent.width, height: qr.extent.height))
("扫码打开 Helmp" as NSString).draw(
    at: NSPoint(x: 420, y: 170),
    withAttributes: [.font: NSFont.systemFont(ofSize: 56), .foregroundColor: NSColor.black])
NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: output))
print("wrote \(output)")
