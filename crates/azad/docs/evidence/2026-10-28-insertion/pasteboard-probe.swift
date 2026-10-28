import AppKit
import Foundation

// This probe uses only a pasteboard that it creates. It never posts input events.
let args = CommandLine.arguments
if args.count == 3 && args[1] == "--read" {
  let board = NSPasteboard(name: NSPasteboard.Name(args[2]))
  let value = board.string(forType: .string)
  let data = try JSONSerialization.data(withJSONObject: ["text": value ?? "", "count": board.changeCount])
  FileHandle.standardOutput.write(data)
  exit(0)
}

let board = NSPasteboard.withUniqueName()
defer { board.releaseGlobally() }
let samples = ["Does it require a special language server?", "These are typically difficult things.", "Unicode 🧠 e\u{301}"]
var mismatches = 0
var crossProcessMismatches = 0
let start = DispatchTime.now().uptimeNanoseconds
for index in 0..<1000 {
  autoreleasepool {
    let expected = samples[index % samples.count]
    board.clearContents()
    let wrote = board.writeObjects([expected as NSString])
    if !wrote || board.string(forType: .string) != expected { mismatches += 1 }
    if index < 12 {
      let pipe = Pipe()
      let child = Process()
      child.executableURL = URL(fileURLWithPath: args[0])
      child.arguments = ["--read", board.name.rawValue]
      child.standardOutput = pipe
      try! child.run()
      let data = pipe.fileHandleForReading.readDataToEndOfFile()
      child.waitUntilExit()
      let result = try! JSONSerialization.jsonObject(with: data) as! [String: Any]
      if child.terminationStatus != 0 || result["text"] as? String != expected { crossProcessMismatches += 1 }
    }
  }
}
let elapsed = Double(DispatchTime.now().uptimeNanoseconds - start) / 1_000_000
print("private_pasteboard_writes=1000 immediate_mismatches=\(mismatches) cross_process_reads=12 cross_process_mismatches=\(crossProcessMismatches) added_delay_ms=0 elapsed_ms=\(elapsed)")
exit(mismatches == 0 && crossProcessMismatches == 0 ? 0 : 1)
