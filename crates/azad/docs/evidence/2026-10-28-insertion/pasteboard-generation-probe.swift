import AppKit
import Foundation

let args = CommandLine.arguments
let prefix = "ai.azad.generation."
if args.count == 3 && args[1] == "--read" {
  let name = args[2]
  guard name.hasPrefix(prefix), UUID(uuidString: String(name.dropFirst(prefix.count))) != nil else {
    fatalError("only a private fixture pasteboard is permitted")
  }
  let board = NSPasteboard(name: NSPasteboard.Name(name))
  let text = board.string(forType: .string)
  let data = try JSONSerialization.data(withJSONObject: [
    "count": board.changeCount, "text": text as Any? ?? NSNull(),
    "types": (board.types ?? []).map(\.rawValue)
  ])
  FileHandle.standardOutput.write(data)
  exit(0)
}
guard args.count == 1 else { fatalError("unexpected fixture arguments") }
let board = NSPasteboard(name: NSPasteboard.Name(prefix + UUID().uuidString))

func readFromChild() throws -> [String: Any] {
  let child = Process()
  let pipe = Pipe()
  child.executableURL = URL(fileURLWithPath: args[0])
  child.arguments = ["--read", board.name.rawValue]
  child.standardOutput = pipe
  try child.run()
  let data = pipe.fileHandleForReading.readDataToEndOfFile()
  child.waitUntilExit()
  guard child.terminationStatus == 0 else { fatalError("private pasteboard read failed") }
  return try JSONSerialization.jsonObject(with: data) as! [String: Any]
}

board.clearContents()
precondition(board.writeObjects(["old fixture" as NSString]))
let before = try readFromChild()
let claimed = board.clearContents()
let gap = try readFromChild()
precondition(board.writeObjects(["new fixture" as NSString]))
let after = try readFromChild()
let exposedGap = gap["count"] as? Int == claimed && gap["text"] is NSNull
let sameGeneration = after["count"] as? Int == claimed
let correctAfter = after["text"] as? String == "new fixture"
let result: [String: Any] = [
  "before": before, "claimed_count": claimed, "gap": gap, "after": after,
  "empty_gap_observed": exposedGap, "same_generation_after_write": sameGeneration,
  "correct_text_after_write": correctAfter, "general_pasteboard_access": false,
  "input_events_posted": false
]
print(String(data: try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys]), encoding: .utf8)!)
board.releaseGlobally()
exit(exposedGap && sameGeneration && correctAfter ? 0 : 1)
