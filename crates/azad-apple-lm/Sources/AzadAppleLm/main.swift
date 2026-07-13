import Foundation

// Line-delimited JSON helper for Hey Azad.
// When FoundationModels is available (newer SDKs / macOS), probes availability and
// can interpret queries. Older toolchains compile the fallback path only.

struct Out: Encodable {
    var state: String? = nil
    var detail: String? = nil
    var ok: Bool? = nil
    var intent: IntentPayload? = nil
    var error: String? = nil
    var message: String? = nil
}

struct IntentPayload: Encodable {
    var kind: String
    var setting: String? = nil
    var enabled: Bool? = nil
    var action: String? = nil
    var word: String? = nil
    var message: String? = nil
}

func writeJSON(_ value: Out) {
    let encoder = JSONEncoder()
    encoder.outputFormatting = []
    if let data = try? encoder.encode(value), let line = String(data: data, encoding: .utf8) {
        print(line)
        fflush(stdout)
    }
}

func probeAvailability() -> Out {
    #if canImport(FoundationModels)
    if #available(macOS 26.0, *) {
        // Dynamic import avoids hard-linking on older SDKs used for the app deploy target.
        // When building with a modern SDK, map SystemLanguageModel.default.availability.
        return mapFoundationModelsAvailability()
    }
    #endif
    return Out(
        state: "unavailable",
        detail: "FoundationModels not available on this OS/SDK"
    )
}

#if canImport(FoundationModels)
import FoundationModels

@available(macOS 26.0, *)
func mapFoundationModelsAvailability() -> Out {
    let model = SystemLanguageModel.default
    switch model.availability {
    case .available:
        return Out(state: "available")
    case .unavailable(let reason):
        switch reason {
        case .appleIntelligenceNotEnabled:
            return Out(state: "appleIntelligenceNotEnabled")
        case .modelNotReady:
            return Out(state: "modelNotReady")
        case .deviceNotEligible:
            return Out(state: "deviceNotEligible")
        default:
            return Out(state: "unavailable", detail: String(describing: reason))
        }
    @unknown default:
        return Out(state: "unavailable", detail: "unknown availability")
    }
}
#endif

func heuristicIntent(query: String) -> IntentPayload {
    let q = query.lowercased()
    func enabledFlag() -> Bool? {
        let disable = ["disable", "turn off", "switch off", "stop", "don't", "do not", "no more"]
        let enable = ["enable", "turn on", "switch on", "start", "activate"]
        if disable.contains(where: { q.contains($0) }) { return false }
        if enable.contains(where: { q.contains($0) }) { return true }
        return nil
    }
    if let en = enabledFlag() {
        let settings: [(String, [String])] = [
            ("convert_number_words", ["number", "numbers"]),
            ("convert_spoken_emoji", ["emoji"]),
            ("remove_hesitations", ["hesitation", "hesitations"]),
            ("trailing_space", ["trailing space", "append space"]),
            ("deduplicate_words", ["repeated", "duplicate", "deduplicate"]),
            ("lowercase_except_uppercase", ["lowercase", "casing"]),
        ]
        for (id, keys) in settings {
            if keys.contains(where: { q.contains($0) }) {
                return IntentPayload(kind: "set_text_setting", setting: id, enabled: en)
            }
        }
    }
    return IntentPayload(
        kind: "unsupported",
        message: "I can change text-replacement settings: numbers, emoji, hesitations, trailing space, repeated words, casing, or removed words."
    )
}

func handleLine(_ line: String) {
    guard let data = line.data(using: .utf8),
          let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
          let cmd = obj["cmd"] as? String
    else {
        writeJSON(Out(error: "invalid request"))
        return
    }
    switch cmd {
    case "availability":
        writeJSON(probeAvailability())
    case "interpret":
        let query = (obj["query"] as? String) ?? ""
        // Always return a closed-catalog intent. Availability is reported separately;
        // the Rust host decides setup UX vs applying the tool.
        let intent = heuristicIntent(query: query)
        writeJSON(Out(ok: true, intent: intent))
    default:
        writeJSON(Out(error: "unknown cmd"))
    }
}

// One-shot: read a single line from stdin, respond, exit.
if let line = readLine() {
    handleLine(line.trimmingCharacters(in: .whitespacesAndNewlines))
} else {
    writeJSON(Out(error: "empty stdin"))
}
