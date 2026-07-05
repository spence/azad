import AppKit
import AzadUI
import CoreGraphics
import Darwin
import Foundation

enum PreviewSurface: String {
    case onboardingFresh = "onboarding-fresh"
    case onboardingReady = "onboarding-ready"
    case settingsGeneral = "settings-general"
    case settingsText = "settings-text"
    case settingsModels = "settings-models"
    case settingsPermissions = "settings-permissions"
    case settingsDebug = "settings-debug"
    case settingsConnectors = "settings-connectors"
    case menuCollapsed = "menu-collapsed"
    case menuExpanded = "menu-expanded"

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
        case .settingsText:
            return .text
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

    var isMenu: Bool {
        switch self {
        case .menuCollapsed, .menuExpanded:
            return true
        default:
            return false
        }
    }
}

struct PreviewOptions {
    var surface: PreviewSurface = .onboardingReady
    var appearance: NSAppearance.Name = .darkAqua
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
        case "--appearance":
            if let value = args.first {
                args.removeFirst()
                options.appearance = value == "light" ? .aqua : .darkAqua
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
            print("usage: azad-ui-preview [--surface <name>] [--appearance light|dark] [--screenshot <path>] [--quit-after <seconds>]")
            print("surfaces: onboarding-fresh, onboarding-ready, settings-general, settings-text, settings-models, settings-permissions, settings-debug, settings-connectors, menu-collapsed, menu-expanded")
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
app.appearance = NSAppearance(named: options.appearance)

func modelPack(status: ModelStatus, progress: UInt8 = 0) -> ModelPack {
    ModelPack(
        id: "nemotron-3.5-mlx-bf16-v1",
        welcomeName: "Nemotron-3.5 ASR Streaming",
        settingsName: "NVIDIA Nemotron-3.5 ASR Streaming 0.6B",
        pageUrl: "https://huggingface.co/mlx-community/nemotron-3.5-asr-streaming-0.6b",
        description: "On-device streaming speech-to-text · English",
        sizeLabel: "1.2 GB",
        status: status,
        downloadPaused: false,
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
        startupListenModeIndex: 2,
        activationLevel: 0,
        pasteMethodIndex: 0,
        autoSubmitIndex: 0,
        overlayPositionIndex: 0,
        appendTrailingSpaceOnPaste: true,
        deduplicateWordsOnPaste: false,
        convertNumberWordsOnPaste: false,
        convertSpokenEmojiOnPaste: false,
        lowercaseExceptUppercaseWordsOnPaste: false,
        removeHesitationsOnPaste: true,
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
        removedWords: ["like", "actually", "basically", "literally", "right", "okay"],
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
if options.surface.isMenu {
    showMenuPreview(expanded: options.surface == .menuExpanded)
} else if options.surface.isOnboarding {
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

class FlippedView: NSView {
    override var isFlipped: Bool { true }
}

final class MenuPreviewPanel: FlippedView {
    private let expanded: Bool
    private let panelWidth: CGFloat
    private let rowHeight: CGFloat = 24
    private let headerHeight: CGFloat = 28
    private let leading: CGFloat = 14
    private let iconSize: CGFloat = 16
    private let labelOffsetY: CGFloat = 1
    private static let devices = [
        "Loop120 by Shokz",
        "BlackHole 2ch",
        "daedalus Microphone",
        "LG UltraFine Display Audio",
        "MacBook Pro Microphone",
    ]

    init(expanded: Bool) {
        self.expanded = expanded
        self.panelWidth = Self.width(expanded: expanded)
        let height: CGFloat = expanded ? 296 : 168
        super.init(frame: NSRect(x: 0, y: 0, width: panelWidth, height: height))
        wantsLayer = true
        layer?.backgroundColor = NSColor(calibratedWhite: 0.055, alpha: 1).cgColor
        layer?.cornerRadius = 12
        layer?.borderColor = NSColor(calibratedWhite: 1, alpha: 0.32).cgColor
        layer?.borderWidth = 1
        build()
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    private func build() {
        add(label("Listen", x: 16, y: 22, width: 160, size: 15, weight: .regular))
        add(switchView(on: true, x: panelWidth - 44, y: 20))
        add(separator(y: 58))

        let deviceTitle = expanded ? "Loop120 by Shokz" : "MacBook Pro Microphone"
        add(symbol("mic", x: leading, y: 72, size: 16))
        add(label(deviceTitle, x: deviceTitleX, y: 71 + labelOffsetY, width: panelWidth - deviceTitleX - 36, size: 14, weight: .semibold))
        add(symbol(expanded ? "chevron.down" : "chevron.right", x: panelWidth - 28, y: 75, size: 10, color: PreviewStyle.secondaryText))

        var bottomSeparatorY: CGFloat = 98
        if expanded {
            var y: CGFloat = 100
            for (idx, device) in Self.devices.enumerated() {
                if idx == 0 {
                    add(symbol("checkmark", x: leading, y: y + labelOffsetY, size: 14, color: PreviewStyle.text))
                }
                add(label(device, x: deviceTitleX, y: y + labelOffsetY, width: panelWidth - deviceTitleX - 18, size: 14, weight: .semibold))
                y += rowHeight
            }
            bottomSeparatorY = y + 6
        }

        add(separator(y: bottomSeparatorY))
        add(label("Settings...", x: 16, y: bottomSeparatorY + 17, width: 170, size: 14, weight: .semibold))
        add(label("Quit Azad", x: 16, y: bottomSeparatorY + 41, width: 170, size: 14, weight: .semibold))
    }

    private var deviceTitleX: CGFloat {
        leading + iconSize + 2
    }

    private static func width(expanded: Bool) -> CGFloat {
        let labels = expanded ? devices : ["MacBook Pro Microphone"]
        let font = NSFont.systemFont(ofSize: 14, weight: .semibold)
        let labelWidth = labels.map { textWidth($0, font: font) }.max() ?? 0
        let deviceTitleX: CGFloat = 32
        let headerWidth = deviceTitleX + labelWidth + 8 + 10 + 12 + 10
        let actionWidth = max(textWidth("Settings...", font: font), textWidth("Quit Azad", font: font)) + 32
        let listenWidth = textWidth("Listen", font: font) + 16 + 10 + 32 + 12 + 10
        return max(220, ceil(max(headerWidth, actionWidth, listenWidth) * 1.10))
    }

    private static func textWidth(_ text: String, font: NSFont) -> CGFloat {
        (text as NSString).size(withAttributes: [.font: font]).width
    }

    private func add(_ view: NSView) {
        addSubview(view)
    }

    private func label(_ text: String, x: CGFloat, y: CGFloat, width: CGFloat, size: CGFloat, weight: NSFont.Weight) -> NSTextField {
        let label = NSTextField(labelWithString: text)
        label.frame = NSRect(x: x, y: y, width: width, height: 20)
        label.font = .systemFont(ofSize: size, weight: weight)
        label.textColor = PreviewStyle.text
        label.backgroundColor = .clear
        label.lineBreakMode = .byTruncatingTail
        return label
    }

    private func symbol(_ name: String, x: CGFloat, y: CGFloat, size: CGFloat, color: NSColor = PreviewStyle.secondaryText) -> NSImageView {
        let config = NSImage.SymbolConfiguration(pointSize: size, weight: .regular)
        let image = NSImage(systemSymbolName: name, accessibilityDescription: nil)?.withSymbolConfiguration(config)
        let view = NSImageView(image: image ?? NSImage())
        view.frame = NSRect(x: x, y: y, width: size + 4, height: size + 4)
        view.contentTintColor = color
        return view
    }

    private func separator(y: CGFloat) -> NSView {
        let view = NSView(frame: NSRect(x: 16, y: y, width: panelWidth - 32, height: 1))
        view.wantsLayer = true
        view.layer?.backgroundColor = NSColor(calibratedWhite: 1, alpha: 0.14).cgColor
        return view
    }

    private func switchView(on: Bool, x: CGFloat, y: CGFloat) -> NSView {
        let view = NSView(frame: NSRect(x: x, y: y, width: 32, height: 18))
        view.wantsLayer = true
        view.layer?.cornerRadius = 9
        view.layer?.backgroundColor = (on ? NSColor(calibratedRed: 0.02, green: 0.52, blue: 0.55, alpha: 1) : PreviewStyle.control).cgColor

        let thumb = NSView(frame: NSRect(x: on ? 16 : 2, y: 2, width: 14, height: 14))
        thumb.wantsLayer = true
        thumb.layer?.cornerRadius = 7
        thumb.layer?.backgroundColor = NSColor.white.cgColor
        view.addSubview(thumb)
        return view
    }
}

enum PreviewStyle {
    static let text = NSColor(calibratedWhite: 0.88, alpha: 1.0)
    static let secondaryText = NSColor(calibratedWhite: 0.62, alpha: 1.0)
    static let control = NSColor(calibratedRed: 0.205, green: 0.205, blue: 0.218, alpha: 1.0)
}

func showMenuPreview(expanded: Bool) {
    let panel = MenuPreviewPanel(expanded: expanded)
    let padding: CGFloat = 12
    let contentSize = NSSize(
        width: panel.frame.width + padding * 2,
        height: panel.frame.height + padding * 2
    )
    let contentView = FlippedView(frame: NSRect(origin: .zero, size: contentSize))
    contentView.wantsLayer = true
    contentView.layer?.backgroundColor = NSColor.clear.cgColor
    panel.frame.origin = NSPoint(x: padding, y: padding)
    contentView.addSubview(panel)

    let window = NSWindow(
        contentRect: NSRect(origin: .zero, size: contentSize),
        styleMask: [.borderless],
        backing: .buffered,
        defer: false
    )
    window.isOpaque = false
    window.backgroundColor = .clear
    window.hasShadow = false
    window.contentView = contentView
    window.center()
    window.makeKeyAndOrderFront(nil)
}

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
