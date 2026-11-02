// The on-screen windows of a process, one per line: "x y w h layer", in
// points from the top left of the main display. Window bounds need no
// screen-recording permission (titles would).
//
//   swiftc -O tests/macos/windows.swift -o windows && ./windows <pid>
import CoreGraphics
import Foundation

let pid = Int32(CommandLine.arguments.dropFirst().first ?? "") ?? -1
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
    as? [[String: Any]] ?? []
for w in list where (w[kCGWindowOwnerPID as String] as? Int32) == pid {
    guard let b = w[kCGWindowBounds as String] as? [String: CGFloat] else { continue }
    let layer = w[kCGWindowLayer as String] as? Int ?? 0
    print(Int(b["X"] ?? 0), Int(b["Y"] ?? 0), Int(b["Width"] ?? 0), Int(b["Height"] ?? 0), layer)
}
