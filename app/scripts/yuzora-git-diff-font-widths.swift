// yuzora(v010): what the system draws each kind of character as wide in the diff's font, in cells
// (the width of an ASCII character), so `bound` in defaults/mods/yuzora-git-diff can be held to it.
// Measured with CoreText, which falls back for what the font lacks as the app does on macOS.
// Run: swift scripts/yuzora-git-diff-font-widths.swift assets/fonts/JetBrainsMono-Variable.ttf 11.5 > scripts/fixtures/yuzora-git-diff-font-widths.json
import CoreText
import Foundation

let args = CommandLine.arguments
guard args.count == 3, let size = Double(args[2]) else {
    FileHandle.standardError.write("usage: yuzora-git-diff-font-widths.swift <font.ttf> <size>\n".data(using: .utf8)!)
    exit(2)
}
let descriptors = CTFontManagerCreateFontDescriptorsFromURL(URL(fileURLWithPath: args[1]) as CFURL) as! [CTFontDescriptor]
let font = CTFontCreateWithFontDescriptor(descriptors[0], CGFloat(size), nil)

func width(_ text: String) -> Double {
    let attributed = NSAttributedString(string: text, attributes: [NSAttributedString.Key(kCTFontAttributeName as String): font])
    return CTLineGetTypographicBounds(CTLineCreateWithAttributedString(attributed as CFAttributedString), nil, nil, nil)
}

let cell = width(String(repeating: "W", count: 40)) / 40
let samples = [
    "W", "i", "@", "~", " ",
    "\u{b7}", "\u{e9}", "\u{17e}", "\u{3a9}", "\u{416}", "\u{428}", "\u{44f}",
    "\u{2014}", "\u{2026}", "\u{201c}", "\u{2022}", "\u{2191}", "\u{2192}", "\u{2193}", "\u{21d2}", "\u{2212}", "\u{2260}",
    "\u{2500}", "\u{251c}", "\u{2588}", "\u{25b6}", "\u{25a0}", "\u{25c6}",
    "\u{2713}", "\u{2714}", "\u{2717}", "\u{2705}", "\u{274c}", "\u{2b50}", "\u{26a1}", "\u{2600}", "\u{2764}", "\u{2611}", "\u{26a0}", "\u{2728}", "\u{2b06}", "\u{279c}", "\u{2715}",
    "\u{231a}", "\u{23e9}", "\u{25fd}", "\u{2460}",
    "\u{6f22}", "\u{5b57}", "\u{3042}", "\u{30ab}", "\u{3001}", "\u{300c}", "\u{ff08}", "\u{ff21}", "\u{ff01}", "\u{ff5e}",
    "\u{d55c}", "\u{314e}",
    "\u{1f600}", "\u{1f680}", "\u{1f44d}",
    "\u{645}", "\u{5d0}", "\u{e01}", "\u{915}",
    "\u{ff71}",
]
let entries = samples.map { text -> String in
    let px = width(text)
    return "    {\"text\": \"\(text)\", \"units\": \(text.utf16.count), \"px\": \(String(format: "%.2f", px)), \"cells\": \(String(format: "%.2f", px / cell))}"
}
print("{\n  \"font\": \"\((args[1] as NSString).lastPathComponent)\",\n  \"size\": \(size),\n  \"cell\": \(String(format: "%.2f", cell)),\n  \"samples\": [\n\(entries.joined(separator: ",\n"))\n  ]\n}")
