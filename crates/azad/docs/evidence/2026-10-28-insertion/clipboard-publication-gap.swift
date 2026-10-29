import AppKit
import Foundation

let model = Process()
let output = Pipe()
model.executableURL = URL(fileURLWithPath: "/usr/sbin/sysctl")
model.arguments = ["-n", "hw.model"]
model.standardOutput = output
try model.run()
let hardware = String(data: output.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
model.waitUntilExit()
guard model.terminationStatus == 0 && hardware.hasPrefix("VirtualMac") else {
  fputs("the publication gap requires an owned macOS VM\n", stderr)
  exit(2)
}
let args = CommandLine.arguments
guard args.count == 3 && ["clear", "declare"].contains(args[1]) else {
  fputs("expected clear or declare and fixture text\n", stderr)
  exit(2)
}
let text = args[2] as NSString
let board = NSPasteboard.general
let claimed: Int
if args[1] == "clear" {
  claimed = board.clearContents()
} else {
  claimed = board.declareTypes([.string], owner: nil)
}
print("gap_open mode=\(args[1]) count=\(claimed) types=\((board.types ?? []).map(\.rawValue))")
fflush(stdout)
// the pause exposes a publication boundary; it is not an insertion delay.
Thread.sleep(forTimeInterval: 1)
let wrote = args[1] == "clear"
  ? board.writeObjects([text])
  : board.setString(args[2], forType: .string)
let matches = board.string(forType: .string) == args[2]
let after = board.changeCount
let result: [String: Any] = [
  "mode": args[1], "claimed_count": claimed, "after_count": after,
  "write_ok": wrote, "clipboard_matches": matches
]
print(String(data: try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys]), encoding: .utf8)!)
exit(wrote && after == claimed && matches ? 0 : 1)
