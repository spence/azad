import Foundation
import MLXAudioSTT

enum WorkerError: Error, CustomStringConvertible {
    case missingValue(String)
    case invalidNumber(String, String)
    case invalidCommand
    case invalidSamples

    var description: String {
        switch self {
        case .missingValue(let flag):
            return "missing value for \(flag)"
        case .invalidNumber(let flag, let value):
            return "invalid number for \(flag): \(value)"
        case .invalidCommand:
            return "invalid command"
        case .invalidSamples:
            return "invalid samples payload"
        }
    }
}

struct Options {
    var modelDir: URL?
    var language = "en-US"
    var streamingChunkMs = 80
    var finalChunkMs = 560

    static func parse(_ args: [String]) throws -> Options {
        var options = Options()
        var index = 1

        func value(after flag: String) throws -> String {
            guard index + 1 < args.count else { throw WorkerError.missingValue(flag) }
            index += 1
            return args[index]
        }

        while index < args.count {
            let flag = args[index]
            switch flag {
            case "--model-dir":
                options.modelDir = URL(fileURLWithPath: try value(after: flag), isDirectory: true)
            case "--language":
                options.language = try value(after: flag)
            case "--streaming-chunk-ms":
                let raw = try value(after: flag)
                guard let value = Int(raw), value > 0 else {
                    throw WorkerError.invalidNumber(flag, raw)
                }
                options.streamingChunkMs = value
            case "--final-chunk-ms":
                let raw = try value(after: flag)
                guard let value = Int(raw), value > 0 else {
                    throw WorkerError.invalidNumber(flag, raw)
                }
                options.finalChunkMs = value
            default:
                throw WorkerError.invalidNumber("argument", flag)
            }
            index += 1
        }

        guard options.modelDir != nil else { throw WorkerError.missingValue("--model-dir") }
        return options
    }
}

final class Worker {
    private let model: NemotronASRModel
    private let language: String
    private let streamingChunkMs: Int
    private let finalChunkMs: Int
    private var liveSession: NemotronASRStreamSession
    private var turnSamples: [Float] = []

    init(options: Options) throws {
        let modelDir = options.modelDir!
        self.model = try NemotronASRModel.fromDirectory(modelDir)
        self.language = options.language
        self.streamingChunkMs = options.streamingChunkMs
        self.finalChunkMs = options.finalChunkMs
        self.liveSession = model.makeStreamSession(
            language: options.language,
            chunkMs: options.streamingChunkMs
        )
    }

    func run() {
        writeResponse(["type": "ready", "ok": true])
        while let line = readLine(strippingNewline: true) {
            do {
                guard let data = line.data(using: .utf8),
                      let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                      let type = object["type"] as? String
                else {
                    throw WorkerError.invalidCommand
                }

                switch type {
                case "chunk":
                    let samples = try parseSamples(object["samples"])
                    let delta = step(samples)
                    writeResponse(["ok": true, "delta": delta])
                case "reset":
                    reset()
                    writeResponse(["ok": true])
                case "finish":
                    let text = finish()
                    writeResponse(["ok": true, "text": text])
                case "shutdown":
                    writeResponse(["ok": true])
                    return
                default:
                    throw WorkerError.invalidCommand
                }
            } catch {
                writeResponse(["ok": false, "error": String(describing: error)])
            }
        }
    }

    private func step(_ samples: [Float]) -> String {
        turnSamples.append(contentsOf: samples)
        return liveSession.step(samples).text
    }

    private func reset() {
        liveSession = model.makeStreamSession(language: language, chunkMs: streamingChunkMs)
        turnSamples.removeAll(keepingCapacity: true)
    }

    private func finish() -> String {
        let streamTail = liveSession.finish().text

        guard !turnSamples.isEmpty else {
            reset()
            return streamTail.trimmingCharacters(in: .whitespacesAndNewlines)
        }

        let finalSession = model.makeStreamSession(language: language, chunkMs: finalChunkMs)
        _ = finalSession.step(turnSamples)
        _ = finalSession.finish()
        let text = finalSession.text.trimmingCharacters(in: .whitespacesAndNewlines)
        reset()
        return text.isEmpty ? streamTail.trimmingCharacters(in: .whitespacesAndNewlines) : text
    }
}

func parseSamples(_ value: Any?) throws -> [Float] {
    guard let raw = value as? [Any] else { throw WorkerError.invalidSamples }
    var samples: [Float] = []
    samples.reserveCapacity(raw.count)
    for item in raw {
        if let number = item as? NSNumber {
            samples.append(number.floatValue)
        } else if let double = item as? Double {
            samples.append(Float(double))
        } else {
            throw WorkerError.invalidSamples
        }
    }
    return samples
}

func writeResponse(_ object: [String: Any]) {
    guard JSONSerialization.isValidJSONObject(object),
          let data = try? JSONSerialization.data(withJSONObject: object),
          let newline = "\n".data(using: .utf8)
    else {
        return
    }
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(newline)
}

do {
    let options = try Options.parse(CommandLine.arguments)
    let worker = try Worker(options: options)
    worker.run()
} catch {
    writeResponse(["type": "ready", "ok": false, "error": String(describing: error)])
    exit(1)
}
