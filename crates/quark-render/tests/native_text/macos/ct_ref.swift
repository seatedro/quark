// CoreText reference renderer for quark's native text rasterizer.
//
//   ct_ref render MANIFEST_DIR OUT_DIR
//       Draws every sheet of swash-dump's manifest with CTFontDrawGlyphs:
//       the same font bytes, face index, variation coordinates, glyph ids,
//       and quantized device positions quark uses. Writes
//       OUT_DIR/<sheet>.<mode>.png (8-bit DeviceGray, the context's pixels)
//       and OUT_DIR/env.json (OS build, smoothing preferences, and the
//       variation instance CoreText resolved for each font).
//
//   ct_ref panel MANIFEST_DIR SHEET COLUMNS
//       Opens a window drawing the sheet on screen through AppKit's own
//       context, black on white and white on black, once per column in
//       COLUMNS (comma-separated: default, off, on, ctline; see PanelView),
//       and writes the block rects to /tmp/ct_ref_panel_layout.txt. A
//       capture of this window is the "what macOS shows" input to the
//       smoothing calibration.
//
// Every offscreen context antialiases, positions glyphs at fractional
// offsets, and disables subpixel quantization (quark already quantizes x to
// quarter pixels). Font smoothing is set explicitly per mode.

import AppKit
import CoreGraphics
import CoreText
import Foundation
import ImageIO

struct Mode {
    let name: String
    /// Text and background gray levels in the context's encoding (0 black).
    let fg: CGFloat
    let bg: CGFloat
    let smooth: Bool
    /// Leave optical size to CoreText (AppKit's behavior) instead of
    /// freezing every axis at the coordinates swash rasterized with.
    let autoOpsz: Bool
}

func fontModes(sheet: String) -> [Mode] {
    var modes = [
        Mode(name: "light-smooth", fg: 0, bg: 1, smooth: true, autoOpsz: false),
        Mode(name: "light-plain", fg: 0, bg: 1, smooth: false, autoOpsz: false),
        Mode(name: "dark-smooth", fg: 1, bg: 0, smooth: true, autoOpsz: false),
        Mode(name: "dark-plain", fg: 1, bg: 0, smooth: false, autoOpsz: false),
    ]
    if sheet.hasPrefix("sf-pro") {
        modes.append(Mode(name: "light-smooth-auto", fg: 0, bg: 1, smooth: true, autoOpsz: true))
        modes.append(Mode(name: "light-plain-auto", fg: 0, bg: 1, smooth: false, autoOpsz: true))
    }
    return modes
}

/// Foreground/background pairs at least half the range apart, so coverage
/// can be recovered as (C - B) / (F - B) with at most 2x quantization.
func sweepModes() -> [Mode] {
    let pairs: [(CGFloat, CGFloat)] = [
        (0, 0.5), (0, 0.75), (0, 1), (0.25, 0.75), (0.25, 1), (0.5, 0),
        (0.5, 1), (0.75, 0), (0.75, 0.25), (1, 0), (1, 0.25), (1, 0.5),
    ]
    var modes = pairs.map { fg, bg in
        Mode(name: "fg\(Int(fg * 100))-bg\(Int(bg * 100))-smooth", fg: fg, bg: bg, smooth: true, autoOpsz: false)
    }
    modes.append(Mode(name: "fg0-bg100-plain", fg: 0, bg: 1, smooth: false, autoOpsz: false))
    modes.append(Mode(name: "fg50-bg100-plain", fg: 0.5, bg: 1, smooth: false, autoOpsz: false))
    modes.append(Mode(name: "fg100-bg0-plain", fg: 1, bg: 0, smooth: false, autoOpsz: false))
    return modes
}

func fail(_ message: String) -> Never {
    FileHandle.standardError.write((message + "\n").data(using: .utf8)!)
    exit(1)
}

func tagValue(_ tag: String) -> Int {
    tag.utf8.reduce(0) { ($0 << 8) | Int($1) }
}

func tagString(_ value: Int) -> String {
    String(bytes: (0..<4).reversed().map { UInt8((value >> ($0 * 8)) & 0xff) }, encoding: .ascii) ?? "\(value)"
}

final class Fonts {
    let dir: URL
    let entries: [[String: Any]]
    var descriptors: [String: [CTFontDescriptor]] = [:]
    var cache: [String: CTFont] = [:]

    init(dir: URL, entries: [[String: Any]]) {
        self.dir = dir
        self.entries = entries
    }

    /// The face swash rasterized, from its source bytes, at `size` points.
    func font(_ index: Int, size: CGFloat, autoOpsz: Bool) -> CTFont {
        let key = "\(index)/\(size)/\(autoOpsz)"
        if let font = cache[key] { return font }
        let entry = entries[index]
        let file = entry["file"] as! String
        let expected = entry["post_script"] as! String
        let base = face(file: file, index: entry["index"] as! Int, postScript: expected)
        var variation: [NSNumber: NSNumber] = [:]
        for axis in entry["axes"] as! [[String: Any]] {
            let tag = axis["tag"] as! String
            if autoOpsz && tag != "wght" { continue }
            variation[NSNumber(value: tagValue(tag))] = axis["value"] as? NSNumber
        }
        var attributes: [CFString: Any] = [:]
        if !variation.isEmpty { attributes[kCTFontVariationAttribute] = variation }
        if !autoOpsz { attributes[kCTFontOpticalSizeAttribute] = "none" }
        let descriptor = CTFontDescriptorCreateCopyWithAttributes(base, attributes as CFDictionary)
        let font = CTFontCreateWithFontDescriptor(descriptor, size, nil)
        cache[key] = font
        return font
    }

    /// The base descriptor of face `index` in a font file's bytes. A single
    /// font's bytes give one descriptor (the plural API would list its named
    /// instances instead); a collection's member is the descriptor carrying
    /// the member's PostScript name, which must be unique.
    func face(file: String, index: Int, postScript: String) -> CTFontDescriptor {
        let key = "\(file)#\(index)"
        if let face = descriptors[key]?.first { return face }
        guard let data = try? Data(contentsOf: dir.appendingPathComponent(file)) else { fail("cannot load \(file)") }
        let face: CTFontDescriptor
        if data.prefix(4) != Data("ttcf".utf8) {
            guard index == 0, let single = CTFontManagerCreateFontDescriptorFromData(data as CFData) else {
                fail("\(file) has no face \(index)")
            }
            face = single
        } else {
            let all = CTFontManagerCreateFontDescriptorsFromData(data as CFData) as? [CTFontDescriptor] ?? []
            let named = all.filter { CTFontDescriptorCopyAttribute($0, kCTFontNameAttribute) as? String == postScript }
            guard named.count == 1 else { fail("\(file): \(named.count) faces named \(postScript)") }
            face = named[0]
        }
        let actual = CTFontDescriptorCopyAttribute(face, kCTFontNameAttribute) as? String ?? ""
        if actual != postScript { fail("\(file)#\(index): CoreText face \(actual) is not \(postScript)") }
        descriptors[key] = [face]
        return face
    }
}

func variationJSON(_ font: CTFont) -> [String: Double] {
    var out: [String: Double] = [:]
    if let variation = CTFontCopyVariation(font) as? [NSNumber: NSNumber] {
        for (tag, value) in variation { out[tagString(tag.intValue)] = value.doubleValue }
    }
    return out
}

func configure(_ ctx: CGContext, smooth: Bool) {
    ctx.setAllowsAntialiasing(true)
    ctx.setShouldAntialias(true)
    ctx.setAllowsFontSmoothing(true)
    ctx.setShouldSmoothFonts(smooth)
    ctx.setAllowsFontSubpixelPositioning(true)
    ctx.setShouldSubpixelPositionFonts(true)
    ctx.setAllowsFontSubpixelQuantization(false)
    ctx.setShouldSubpixelQuantizeFonts(false)
    ctx.textMatrix = .identity
}

/// Draws a sheet's glyphs into `ctx`, whose user space is y-up logical
/// points over a sheet `height` device pixels tall. A glyph's device
/// position is its integer x plus its quarter-pixel bin, on its integer
/// baseline, so CoreText rasterizes the same offsets swash did.
func drawSheet(_ sheet: [String: Any], fonts: Fonts, ctx: CGContext, height: CGFloat, autoOpsz: Bool) {
    let scale = CGFloat(sheet["scale"] as! Int)
    for case_ in sheet["cases"] as! [[String: Any]] {
        let size = CGFloat((case_["size"] as! NSNumber).doubleValue)
        for g in case_["glyphs"] as! [[Any]] {
            let font = fonts.font(g[0] as! Int, size: size, autoOpsz: autoOpsz)
            var glyph = CGGlyph((g[1] as! NSNumber).intValue)
            let x = CGFloat((g[2] as! NSNumber).doubleValue) + CGFloat((g[3] as! NSNumber).doubleValue)
            let y = CGFloat((g[4] as! NSNumber).doubleValue)
            var position = CGPoint(x: x / scale, y: (height - y) / scale)
            CTFontDrawGlyphs(font, &glyph, &position, 1, ctx)
        }
    }
}

func writePNG(_ image: CGImage, to url: URL) {
    guard let dest = CGImageDestinationCreateWithURL(url as CFURL, "public.png" as CFString, 1, nil) else {
        fail("cannot write \(url.path)")
    }
    CGImageDestinationAddImage(dest, image, nil)
    if !CGImageDestinationFinalize(dest) { fail("cannot finalize \(url.path)") }
}

func loadManifest(_ dir: URL) -> [String: Any] {
    guard let data = try? Data(contentsOf: dir.appendingPathComponent("manifest.json")),
          let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
    else { fail("cannot read \(dir.path)/manifest.json") }
    return json
}

func preference(_ key: String) -> Any {
    CFPreferencesCopyAppValue(key as CFString, kCFPreferencesAnyApplication) ?? NSNull()
}

func render(manifestDir: URL, outDir: URL) {
    let manifest = loadManifest(manifestDir)
    let fonts = Fonts(dir: manifestDir, entries: manifest["fonts"] as! [[String: Any]])
    try? FileManager.default.createDirectory(at: outDir, withIntermediateDirectories: true)
    let gray = CGColorSpaceCreateDeviceGray()
    for sheet in manifest["sheets"] as! [[String: Any]] {
        let id = sheet["id"] as! String
        let size = sheet["size"] as! [Int]
        let modes = (sheet["kind"] as! String) == "sweep" ? sweepModes() : fontModes(sheet: id)
        for mode in modes {
            guard let ctx = CGContext(
                data: nil, width: size[0], height: size[1], bitsPerComponent: 8, bytesPerRow: 0,
                space: gray, bitmapInfo: CGImageAlphaInfo.none.rawValue)
            else { fail("no context for \(id)") }
            ctx.setFillColor(gray: mode.bg, alpha: 1)
            ctx.fill(CGRect(x: 0, y: 0, width: size[0], height: size[1]))
            let scale = CGFloat(sheet["scale"] as! Int)
            ctx.scaleBy(x: scale, y: scale)
            configure(ctx, smooth: mode.smooth)
            ctx.setFillColor(gray: mode.fg, alpha: 1)
            drawSheet(sheet, fonts: fonts, ctx: ctx, height: CGFloat(size[1]), autoOpsz: mode.autoOpsz)
            writePNG(ctx.makeImage()!, to: outDir.appendingPathComponent("\(id).\(mode.name).png"))
        }
    }
    // What CoreText resolved, per font entry and size, in both opsz modes.
    var resolved: [[String: Any]] = []
    for (key, font) in fonts.cache.sorted(by: { $0.key < $1.key }) {
        resolved.append([
            "key": key,
            "post_script": CTFontCopyPostScriptName(font) as String,
            "variation": variationJSON(font),
        ])
    }
    var system: [String: Any] = [:]
    if let ui = CTFontCreateUIFontForLanguage(.system, 13, nil) {
        system = ["post_script": CTFontCopyPostScriptName(ui) as String, "variation": variationJSON(ui)]
    }
    let env: [String: Any] = [
        "os": ProcessInfo.processInfo.operatingSystemVersionString,
        "AppleFontSmoothing": preference("AppleFontSmoothing"),
        "CGFontRenderingFontSmoothingDisabled": preference("CGFontRenderingFontSmoothingDisabled"),
        "color_space": "DeviceGray",
        "system_ui_13pt": system,
        "fonts": resolved,
    ]
    let data = try! JSONSerialization.data(withJSONObject: env, options: [.prettyPrinted, .sortedKeys])
    try! data.write(to: outDir.appendingPathComponent("env.json"))
}

// MARK: - On-screen panel

/// The sheet drawn once per (background, column) block, blocks laid out in
/// a grid that fits the 1024x768 macbox display. Columns: `default`
/// (AppKit's context state untouched), `off` and `on` (smoothing forced),
/// `ctline` (the string laid out by CTLine with the system UI font). Light
/// blocks (black on white) come first, then dark ones. Block i's view
/// rect is listed in the window title order by `ct_ref panel` on stdout.
final class PanelView: NSView {
    let sheet: [String: Any]
    let fonts: Fonts
    let text: String
    let columns: [String]
    let blockSize: CGSize
    let perRow: Int
    var blocks: [(rect: CGRect, column: String, fg: CGFloat, bg: CGFloat)] = []

    init(sheet: [String: Any], fonts: Fonts, text: String, columns: [String]) {
        self.sheet = sheet
        self.fonts = fonts
        self.text = text
        self.columns = columns
        let size = sheet["size"] as! [Int]
        blockSize = CGSize(width: size[0], height: size[1])
        perRow = max(1, Int(1000 / (blockSize.width + 8)))
        let count = columns.count * 2
        let rows = (count + perRow - 1) / perRow
        let frame = NSRect(
            x: 0, y: 0, width: CGFloat(min(count, perRow)) * (blockSize.width + 8) + 8,
            height: CGFloat(rows) * (blockSize.height + 8) + 8)
        super.init(frame: frame)
        for (i, (fg, bg)) in [(CGFloat(0), CGFloat(1)), (CGFloat(1), CGFloat(0))].enumerated() {
            for (j, column) in columns.enumerated() {
                let n = i * columns.count + j
                let x = 8 + CGFloat(n % perRow) * (blockSize.width + 8)
                let top = 8 + CGFloat(n / perRow) * (blockSize.height + 8)
                let rect = CGRect(x: x, y: frame.height - top - blockSize.height, width: blockSize.width, height: blockSize.height)
                blocks.append((rect, column, fg, bg))
            }
        }
    }

    required init?(coder: NSCoder) { fatalError() }

    override var isOpaque: Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        // Mid gray between blocks, so a block's edge is unambiguous.
        ctx.setFillColor(gray: 0.5, alpha: 1)
        ctx.fill(bounds)
        for block in blocks {
            ctx.saveGState()
            ctx.setFillColor(gray: block.bg, alpha: 1)
            ctx.fill(block.rect)
            ctx.clip(to: block.rect)
            ctx.translateBy(x: block.rect.minX, y: block.rect.minY)
            ctx.setFillColor(gray: block.fg, alpha: 1)
            ctx.textMatrix = .identity
            // Positions are quark's quantized ones in every column; only
            // smoothing is left to AppKit in `default`.
            ctx.setAllowsFontSubpixelPositioning(true)
            ctx.setShouldSubpixelPositionFonts(true)
            ctx.setAllowsFontSubpixelQuantization(false)
            ctx.setShouldSubpixelQuantizeFonts(false)
            switch block.column {
            case "off": ctx.setShouldSmoothFonts(false)
            case "on":
                ctx.setAllowsFontSmoothing(true)
                ctx.setShouldSmoothFonts(true)
            default: break
            }
            if block.column == "ctline" {
                drawLines(ctx, fg: block.fg)
            } else {
                drawSheet(sheet, fonts: fonts, ctx: ctx, height: blockSize.height, autoOpsz: false)
            }
            ctx.restoreGState()
        }
    }

    /// The sheet's lines laid out by CTLine with the system UI font at each
    /// line's size and weight: shaping, fallback, and optical size as AppKit
    /// would choose them.
    func drawLines(_ ctx: CGContext, fg: CGFloat) {
        for case_ in sheet["cases"] as! [[String: Any]] {
            let size = CGFloat((case_["size"] as! NSNumber).doubleValue)
            let weight = (case_["weight"] as! NSNumber).doubleValue
            let mono = (case_["font"] as! String) == "sf-mono"
            let weights: [Double: NSFont.Weight] = [300: .light, 400: .regular, 500: .medium, 600: .semibold, 700: .bold]
            let nsWeight = weights[weight] ?? .regular
            let font = mono
                ? NSFont.monospacedSystemFont(ofSize: size, weight: nsWeight)
                : NSFont.systemFont(ofSize: size, weight: nsWeight)
            let first = (case_["glyphs"] as! [[Any]])[0]
            let x = CGFloat((first[2] as! NSNumber).doubleValue)
            let y = blockSize.height - CGFloat((first[4] as! NSNumber).doubleValue)
            let string = NSAttributedString(
                string: text, attributes: [.font: font, .foregroundColor: NSColor(white: fg, alpha: 1)])
            ctx.textPosition = CGPoint(x: x, y: y)
            CTLineDraw(CTLineCreateWithAttributedString(string), ctx)
        }
    }
}

func panel(manifestDir: URL, sheetID: String, columns: [String]) {
    let manifest = loadManifest(manifestDir)
    let fonts = Fonts(dir: manifestDir, entries: manifest["fonts"] as! [[String: Any]])
    guard let sheet = (manifest["sheets"] as! [[String: Any]]).first(where: { $0["id"] as? String == sheetID }) else {
        fail("no sheet \(sheetID)")
    }
    let app = NSApplication.shared
    app.setActivationPolicy(.regular)
    let view = PanelView(sheet: sheet, fonts: fonts, text: manifest["text"] as! String, columns: columns)
    let window = NSWindow(contentRect: view.frame, styleMask: [.titled], backing: .buffered, defer: false)
    window.title = "ct_ref \(sheetID)"
    window.contentView = view
    // Below the notification banners' corner of the 1024x768 display.
    window.setFrameTopLeftPoint(NSPoint(x: 0, y: NSScreen.main!.visibleFrame.maxY - 150))
    window.makeKeyAndOrderFront(nil)
    // Block rects in view coordinates, top-left origin, for the capture.
    let layout = view.blocks.map { b in
        "\(b.column) \(b.fg == 0 ? "light" : "dark") \(Int(b.rect.minX)) \(Int(view.frame.height - b.rect.maxY)) \(Int(b.rect.width)) \(Int(b.rect.height))"
    }
    try? (layout.joined(separator: "\n") + "\n").write(
        toFile: "/tmp/ct_ref_panel_layout.txt", atomically: true, encoding: .utf8)
    app.activate(ignoringOtherApps: true)
    app.run()
}

let args = CommandLine.arguments
// A bundled .app gets its arguments from a file next to its executable.
var argv = Array(args.dropFirst())
if argv.isEmpty, let packed = try? String(
    contentsOf: URL(fileURLWithPath: args[0]).deletingLastPathComponent().appendingPathComponent("args.txt"), encoding: .utf8)
{
    argv = packed.split(whereSeparator: { $0 == " " || $0 == "\n" }).map(String.init)
}
switch argv.first ?? "" {
case "render" where argv.count == 3:
    render(manifestDir: URL(fileURLWithPath: argv[1]), outDir: URL(fileURLWithPath: argv[2]))
case "panel" where argv.count == 4:
    panel(manifestDir: URL(fileURLWithPath: argv[1]), sheetID: argv[2], columns: argv[3].split(separator: ",").map(String.init))
default:
    fail("usage: ct_ref render MANIFEST_DIR OUT_DIR | ct_ref panel MANIFEST_DIR SHEET COLUMNS")
}
