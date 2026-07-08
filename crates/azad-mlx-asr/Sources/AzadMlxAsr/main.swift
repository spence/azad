import CoreML
import Foundation
import MLXAudioSTT

enum WorkerError: Error, CustomStringConvertible {
  case missingValue(String)
  case invalidNumber(String, String)
  case invalidCommand
  case invalidSamples
  case invalidModel(String)
  case missingModelOutput(String)

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
    case .invalidModel(let message):
      return "invalid CoreML VAD model: \(message)"
    case .missingModelOutput(let name):
      return "CoreML VAD model did not return \(name)"
    }
  }
}

enum WorkerMode {
  case asr
  case vad
}

struct Options {
  var modelDir: URL?
  var vadModel: URL?
  var mode: WorkerMode = .asr
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
      case "--vad-model":
        options.vadModel = URL(fileURLWithPath: try value(after: flag), isDirectory: true)
        options.mode = .vad
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

    switch options.mode {
    case .asr:
      guard options.modelDir != nil else { throw WorkerError.missingValue("--model-dir") }
    case .vad:
      guard options.vadModel != nil else { throw WorkerError.missingValue("--vad-model") }
    }
    return options
  }
}

final class AsrWorker {
  private let model: NemotronASRModel
  private let language: String
  private let streamingChunkMs: Int
  private var liveSession: NemotronASRStreamSession

  init(options: Options) throws {
    let modelDir = options.modelDir!
    self.model = try NemotronASRModel.fromDirectory(modelDir)
    self.language = options.language
    self.streamingChunkMs = options.streamingChunkMs
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
          // `finish` flushes just the streaming session's own tail — no whole-turn re-decode.
          // The dual-stream refined pass is fed continuously, so finalize is a cheap flush of the
          // last buffered chunk, not an O(turn) re-decode.
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
    return liveSession.step(samples).text
  }

  private func reset() {
    liveSession = model.makeStreamSession(language: language, chunkMs: streamingChunkMs)
  }

  private func finish() -> String {
    let streamTail = liveSession.finish().text
    reset()
    return streamTail.trimmingCharacters(in: .whitespacesAndNewlines)
  }
}

final class CoreMLVadWorker {
  private static let frameSamples = 512
  private static let contextSamples = 64
  private static let modelSamples = frameSamples + contextSamples
  private static let stateSize = 128

  private let model: MLModel
  private var h: MLMultiArray
  private var c: MLMultiArray
  private var context: [Float]

  init(modelURL: URL) throws {
    let config = MLModelConfiguration()
    config.computeUnits = .all
    self.model = try MLModel(contentsOf: modelURL, configuration: config)
    try Self.validate(model)
    self.h = try Self.zeros(Self.shape(1, 1, Self.stateSize))
    self.c = try Self.zeros(Self.shape(1, 1, Self.stateSize))
    self.context = Array(repeating: 0, count: Self.contextSamples)
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
        case "vad":
          let samples = try parseSamples(object["samples"])
          let probs = try process(samples)
          let avg = probs.isEmpty ? 0.0 : probs.reduce(0.0, +) / Float(probs.count)
          writeResponse(["ok": true, "probs": probs, "avg": avg])
        case "reset":
          reset()
          writeResponse(["ok": true])
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

  private func process(_ samples: [Float]) throws -> [Float] {
    guard !samples.isEmpty else { return [] }

    var probs: [Float] = []
    probs.reserveCapacity((samples.count + Self.frameSamples - 1) / Self.frameSamples)

    var offset = 0
    while offset < samples.count {
      var frame = Array(repeating: Float(0), count: Self.frameSamples)
      let available = min(Self.frameSamples, samples.count - offset)
      if available > 0 {
        frame.replaceSubrange(0..<available, with: samples[offset..<(offset + available)])
      }
      probs.append(try feed(frame))
      offset += Self.frameSamples
    }

    return probs
  }

  private func feed(_ frame: [Float]) throws -> Float {
    var modelInput = Array(repeating: Float(0), count: Self.modelSamples)
    modelInput.replaceSubrange(0..<Self.contextSamples, with: context)
    modelInput.replaceSubrange(Self.contextSamples..<Self.modelSamples, with: frame)

    let audio = try Self.multiArray(modelInput, shape: Self.shape(1, 1, Self.modelSamples))
    let provider = try MLDictionaryFeatureProvider(dictionary: [
      "audio": audio,
      "h": h,
      "c": c,
    ])
    let output = try model.prediction(from: provider)

    let probability = try Self.output(output, "probability")
    h = try Self.output(output, "h_out")
    c = try Self.output(output, "c_out")
    context = Array(frame[(Self.frameSamples - Self.contextSamples)..<Self.frameSamples])

    return probability[0].floatValue
  }

  private func reset() {
    h = try! Self.zeros(Self.shape(1, 1, Self.stateSize))
    c = try! Self.zeros(Self.shape(1, 1, Self.stateSize))
    context = Array(repeating: 0, count: Self.contextSamples)
  }

  private static func validate(_ model: MLModel) throws {
    let inputs = Set(model.modelDescription.inputDescriptionsByName.keys)
    let outputs = Set(model.modelDescription.outputDescriptionsByName.keys)
    let requiredInputs: Set<String> = ["audio", "h", "c"]
    let requiredOutputs: Set<String> = ["probability", "h_out", "c_out"]
    if !requiredInputs.isSubset(of: inputs) {
      throw WorkerError.invalidModel(
        "missing inputs \(requiredInputs.subtracting(inputs).sorted())")
    }
    if !requiredOutputs.isSubset(of: outputs) {
      throw WorkerError.invalidModel(
        "missing outputs \(requiredOutputs.subtracting(outputs).sorted())")
    }
  }

  private static func zeros(_ shape: [NSNumber]) throws -> MLMultiArray {
    let array = try MLMultiArray(shape: shape, dataType: .float16)
    let ptr = array.dataPointer.bindMemory(to: Float16.self, capacity: array.count)
    for i in 0..<array.count {
      ptr[i] = 0
    }
    return array
  }

  private static func multiArray(_ values: [Float], shape: [NSNumber]) throws -> MLMultiArray {
    let array = try MLMultiArray(shape: shape, dataType: .float16)
    let ptr = array.dataPointer.bindMemory(to: Float16.self, capacity: array.count)
    for i in 0..<min(values.count, array.count) {
      ptr[i] = Float16(values[i])
    }
    return array
  }

  private static func shape(_ values: Int...) -> [NSNumber] {
    values.map { NSNumber(value: $0) }
  }

  private static func output(_ provider: MLFeatureProvider, _ name: String) throws -> MLMultiArray {
    guard let value = provider.featureValue(for: name)?.multiArrayValue else {
      throw WorkerError.missingModelOutput(name)
    }
    return value
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
  switch options.mode {
  case .asr:
    let worker = try AsrWorker(options: options)
    worker.run()
  case .vad:
    let worker = try CoreMLVadWorker(modelURL: options.vadModel!)
    worker.run()
  }
} catch {
  writeResponse(["type": "ready", "ok": false, "error": String(describing: error)])
  exit(1)
}
