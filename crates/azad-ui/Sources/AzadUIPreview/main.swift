import AppKit
import AzadUI
import CoreGraphics
import Darwin
import Foundation

enum PreviewSurface: String {
    case onboardingFresh = "onboarding-fresh"
    case onboardingReady = "onboarding-ready"
    case settingsGeneral = "settings-general"
    case settingsModels = "settings-models"
    case settingsPermissions = "settings-permissions"
    case settingsDebug = "settings-debug"
    case settingsConnectors = "settings-connectors"

    var isOnboarding: Bool {
        switch self {
        case .onboardingFresh, .onboardingReady:
            return true
        default:
            return false
        }
    }

    var settingsTab: SettingsTab {
        switch self {
        case .settingsModels:
            return .models
        case .settingsPermissions:
            return .permissions
        case .settingsDebug:
            return .debug
        case .settingsConnectors:
            return .connectors
        default:
            return .general
        }
    }
}

struct PreviewOptions {
    var surface: PreviewSurface = .onboardingReady
    var screenshotPath: String?
    var quitAfterSeconds: Double = 0.6
}

func parseOptions() -> PreviewOptions {
    var options = PreviewOptions()
    var args = Array(CommandLine.arguments.dropFirst())
    while !args.isEmpty {
        let arg = args.removeFirst()
        switch arg {
        case "--surface":
            if let value = args.first {
                args.removeFirst()
                options.surface = PreviewSurface(rawValue: value) ?? options.surface
            }
        case "--screenshot":
            if let value = args.first {
                args.removeFirst()
                options.screenshotPath = value
            }
        case "--quit-after":
            if let value = args.first {
                args.removeFirst()
                options.quitAfterSeconds = Double(value) ?? options.quitAfterSeconds
            }
        case "--help", "-h":
            print("usage: azad-ui-preview [--surface <name>] [--screenshot <path>] [--quit-after <seconds>]")
            print("surfaces: onboarding-fresh, onboarding-ready, settings-general, settings-models, settings-permissions, settings-debug, settings-connectors")
            exit(0)
        default:
            break
        }
    }
    return options
}

let app = NSApplication.shared
app.setActivationPolicy(.regular)
configurePreviewIcon()

let options = parseOptions()

func modelPack(status: ModelStatus, progress: UInt8 = 0) -> ModelPack {
    ModelPack(
        id: "nemotron-3.5-mlx-bf16-v1",
        welcomeName: "Nemotron-3.5 ASR Streaming",
        settingsName: "NVIDIA Nemotron-3.5 ASR Streaming 0.6B",
        description: "On-device streaming speech-to-text · English",
        sizeLabel: "1.2 GB",
        status: status,
        progressPct: progress,
        bytesDoneLabel: progress > 0 ? "612 MB" : "0 MB",
        bytesTotalLabel: "1.2 GB",
        errorMessage: "Couldn't verify model files"
    )
}

func onboardingModel(ready: Bool) -> OnboardingViewModel {
    OnboardingViewModel(
        alwaysListeningEnabled: true,
        historyEnabled: true,
        pasteMethodIndex: 0,
        appendTrailingSpaceOnPaste: ready,
        overlayPositionIndex: 0,
        runOnStartupEnabled: ready,
        accessibilityStatus: ready ? .granted : .notGranted,
        microphoneStatus: ready ? .granted : .notGranted,
        model: modelPack(status: ready ? .ready : .notDownloaded),
        getStartedEnabled: ready,
        devices: [DeviceOption(id: "default", label: "MacBook Pro Microphone")],
        selectedDeviceIndex: 0,
        listenModifiers: 4
    )
}

func settingsModel(tab: SettingsTab) -> SettingsViewModel {
    SettingsViewModel(
        selectedTab: tab,
        accessibilityStatus: .granted,
        microphoneStatus: tab == .permissions ? .notGranted : .granted,
        runOnStartupEnabled: true,
        pasteMethodIndex: 0,
        autoSubmitIndex: 0,
        overlayPositionIndex: 0,
        appendTrailingSpaceOnPaste: true,
        listenModifiers: 4,
        debugStatsEnabled: true,
        metricsText: """
        session.turns                 128
        asr.rtf.mean                 0.34
        finalize.ms.p50              412
        finalize.ms.p95              980
        insert.paste.ok              126
        gateway.requests              14
        """,
        model: modelPack(status: tab == .models ? .downloading : .ready, progress: tab == .models ? 51 : 100),
        removedWords: ["um", "uh", "like", "you know"],
        connectors: [ConnectorRow(displayName: "Claude", trigger: "hey claude", enabled: true)],
        buildInfo: "build preview · 2026-07-02 19:41"
    )
}

func configurePreviewIcon() {
    let cwd = FileManager.default.currentDirectoryPath
    let candidates = [
        URL(fileURLWithPath: cwd).appendingPathComponent("crates/azad/assets/azad-white.png"),
        URL(fileURLWithPath: cwd).appendingPathComponent("../azad/assets/azad-white.png"),
    ]
    for url in candidates where FileManager.default.fileExists(atPath: url.path) {
        if let image = NSImage(contentsOf: url) {
            app.applicationIconImage = image
            return
        }
    }
}

let encoder = JSONEncoder()
if options.surface.isOnboarding {
    let model = onboardingModel(ready: options.surface == .onboardingReady)
    let json = String(data: try encoder.encode(model), encoding: .utf8)!
    json.withCString { _ = azadUIShowOnboarding($0) }
} else {
    let model = settingsModel(tab: options.surface.settingsTab)
    let json = String(data: try encoder.encode(model), encoding: .utf8)!
    json.withCString { _ = azadUIShowSettings($0) }
}

app.activate(ignoringOtherApps: true)

if let screenshotPath = options.screenshotPath {
    DispatchQueue.main.asyncAfter(deadline: .now() + options.quitAfterSeconds) {
        do {
            try saveFrontWindowScreenshot(to: screenshotPath)
        } catch {
            fputs("azad-ui-preview screenshot failed: \(error)\n", stderr)
            exit(1)
        }
        app.terminate(nil)
    }
}

app.run()

enum ScreenshotError: Error {
    case noVisibleWindow
    case imageUnavailable
    case pngEncodingFailed
}

func saveFrontWindowScreenshot(to path: String) throws {
    guard let window = NSApp.windows.first(where: { $0.isVisible }) else {
        throw ScreenshotError.noVisibleWindow
    }

    let windowImage = captureWindowImage(for: window)
    let image = windowImage ?? renderContentImage(for: window)
    guard let image else {
        throw ScreenshotError.imageUnavailable
    }

    let rep = NSBitmapImageRep(cgImage: image)
    guard let data = rep.representation(using: .png, properties: [:]) else {
        throw ScreenshotError.pngEncodingFailed
    }

    let url = URL(fileURLWithPath: path)
    try FileManager.default.createDirectory(
        at: url.deletingLastPathComponent(),
        withIntermediateDirectories: true
    )
    try data.write(to: url)
}

func captureWindowImage(for window: NSWindow) -> CGImage? {
    typealias CaptureFn = @convention(c) (CGRect, UInt32, UInt32, UInt32) -> Unmanaged<CGImage>?

    guard let handle = dlopen("/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics", RTLD_LAZY) else {
        return nil
    }
    defer { dlclose(handle) }

    guard let symbol = dlsym(handle, "CGWindowListCreateImage") else {
        return nil
    }

    let capture = unsafeBitCast(symbol, to: CaptureFn.self)
    return capture(
        .null,
        CGWindowListOption.optionIncludingWindow.rawValue,
        UInt32(window.windowNumber),
        CGWindowImageOption.boundsIgnoreFraming.rawValue
    )?.takeRetainedValue()
}

func renderContentImage(for window: NSWindow) -> CGImage? {
    guard let contentView = window.contentView else {
        return nil
    }

    let bounds = contentView.bounds
    guard let rep = contentView.bitmapImageRepForCachingDisplay(in: bounds) else {
        return nil
    }
    contentView.cacheDisplay(in: bounds, to: rep)
    return rep.cgImage
}
