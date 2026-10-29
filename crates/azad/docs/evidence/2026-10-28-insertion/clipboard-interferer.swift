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
  fatalError("the clipboard interferer requires an owned macOS VM")
}
guard CommandLine.arguments.count == 3 else { fatalError("expected new text and replacement text") }
let board = NSPasteboard.general
let deadline = Date().addingTimeInterval(10)
print("armed")
fflush(stdout)
while board.string(forType: .string) != CommandLine.arguments[1] {
  guard Date() < deadline else { fatalError("the expected fixture write did not occur") }
  Thread.sleep(forTimeInterval: 0.0005)
}
let before = board.changeCount
board.clearContents()
guard board.writeObjects([CommandLine.arguments[2] as NSString]) else { fatalError("replacement write failed") }
print("replaced before_count=\(before) after_count=\(board.changeCount)")
