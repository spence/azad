import AppKit
import Foundation

// measure AppKit readback on a private pasteboard; no general clipboard or input events.
let board = NSPasteboard.withUniqueName()
defer { board.releaseGlobally() }

func publish(_ text: String) -> Int {
  let generation = board.clearContents()
  precondition(board.writeObjects([text as NSString]))
  return generation
}

// match the production count/read/count sequence and compare exact UTF-8 bytes.
func validate(_ expected: Data, _ generation: Int) -> Bool {
  let before = board.changeCount
  let actual = before == generation ? board.string(forType: .string) : nil
  let after = board.changeCount
  return before == generation && after == generation && actual.map { Data($0.utf8) } == expected
}

func summary(_ samples: [UInt64]) -> [String: Double] {
  let sorted = samples.sorted()
  func percentile(_ fraction: Double) -> Double {
    let index = max(0, Int(ceil(Double(sorted.count) * fraction)) - 1)
    return Double(sorted[index]) / 1000
  }
  return ["p50_us": percentile(0.50), "p95_us": percentile(0.95),
          "p99_us": percentile(0.99), "max_us": Double(sorted.last!) / 1000]
}

// verify successful readback, stale text, stale generation, and unequal UTF-8 encodings.
let expected = Data("new transcript".utf8)
var generation = publish("new transcript")
precondition(validate(expected, generation))
generation = publish("old transcript")
precondition(board.string(forType: .string) == "old transcript")
precondition(!validate(expected, generation))
let staleGeneration = generation
generation = publish("new transcript")
precondition(!validate(expected, staleGeneration))
generation = publish("e\u{301}")
precondition(board.string(forType: .string).map { Data($0.utf8) } == Data("e\u{301}".utf8))
precondition(!validate(Data("\u{e9}".utf8), generation))

// record the first readback and three consecutive readbacks of each private-board write.
var results: [[String: Any]] = []
for size in [40, 338, 4096, 65536] {
  let text = String(repeating: "a", count: size) + " 🧠 e\u{301}"
  let bytes = Data(text.utf8)
  var single: [UInt64] = []
  var triple: [UInt64] = []
  for index in 0..<1050 {
    autoreleasepool {
      let generation = publish(text)
      let start = DispatchTime.now().uptimeNanoseconds
      let first = validate(bytes, generation)
      let firstEnd = DispatchTime.now().uptimeNanoseconds
      let second = validate(bytes, generation)
      let third = validate(bytes, generation)
      let end = DispatchTime.now().uptimeNanoseconds
      precondition(first && second && third)
      if index >= 50 {
        single.append(firstEnd - start)
        triple.append(end - start)
      }
    }
  }
  results.append(["payload_bytes": bytes.count, "samples": single.count,
                  "one_validation": summary(single),
                  "three_back_to_back_validations": summary(triple)])
}
// emit timing summaries; the safety fields describe the fixture, not separate measurements.
let output: [String: Any] = ["private_pasteboard": true, "general_pasteboard_access": false,
                           "input_events": false, "negative_controls": 3, "results": results]
let data = try JSONSerialization.data(
  withJSONObject: output, options: [.prettyPrinted, .sortedKeys])
FileHandle.standardOutput.write(data)
FileHandle.standardOutput.write(Data("\n".utf8))
