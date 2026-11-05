// 生成 OCR 测试图 ocr-zh-en.png：白底黑字，一行中英混排 + 一行英文句子。
// 用法：swift tests/fixtures/make-ocr-fixture.swift tests/fixtures/ocr-zh-en.png
import AppKit

let output = CommandLine.arguments.dropFirst().first ?? "ocr-zh-en.png"
let size = NSSize(width: 1200, height: 320)
let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil, pixelsWide: Int(size.width), pixelsHigh: Int(size.height),
    bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
    colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
NSColor.white.setFill()
NSRect(origin: .zero, size: size).fill()
("你好 Huan 2026" as NSString).draw(
    at: NSPoint(x: 60, y: 190),
    withAttributes: [.font: NSFont.systemFont(ofSize: 64), .foregroundColor: NSColor.black])
("The quick brown fox jumps over the lazy dog" as NSString).draw(
    at: NSPoint(x: 60, y: 60),
    withAttributes: [.font: NSFont.systemFont(ofSize: 44), .foregroundColor: NSColor.black])
NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: output))
print("wrote \(output)")
