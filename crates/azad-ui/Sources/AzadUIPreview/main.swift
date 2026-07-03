import AppKit
import AzadUI
import Foundation

let app = NSApplication.shared
app.setActivationPolicy(.regular)

let model = ModelPack(
    id: "nemotron-3.5-mlx-bf16-v1",
    welcomeName: "Nemotron-3.5 ASR Streaming",
    settingsName: "NVIDIA Nemotron-3.5 ASR Streaming 0.6B",
    description: "On-device streaming speech-to-text · English",
    sizeLabel: "1.2 GB",
    status: .ready,
    progressPct: 100,
    bytesDoneLabel: "1.2 GB",
    bytesTotalLabel: "1.2 GB",
    errorMessage: ""
)

let onboarding = OnboardingViewModel(
    alwaysListeningEnabled: true,
    historyEnabled: true,
    pasteMethodIndex: 0,
    appendTrailingSpaceOnPaste: true,
    overlayPositionIndex: 0,
    runOnStartupEnabled: true,
    accessibilityStatus: .granted,
    microphoneStatus: .granted,
    model: model,
    getStartedEnabled: true,
    devices: [DeviceOption(id: "default", label: "MacBook Pro Microphone")],
    selectedDeviceIndex: 0,
    listenModifiers: 4
)

let settings = SettingsViewModel(
    selectedTab: .general,
    accessibilityStatus: .granted,
    microphoneStatus: .notGranted,
    runOnStartupEnabled: true,
    pasteMethodIndex: 0,
    autoSubmitIndex: 0,
    overlayPositionIndex: 0,
    appendTrailingSpaceOnPaste: true,
    listenModifiers: 4,
    debugStatsEnabled: true,
    metricsText: "session.turns                 128\nasr.rtf.mean                 0.34\nfinalize.ms.p50              412\nfinalize.ms.p95              980\ninsert.paste.ok              126",
    model: model,
    removedWords: ["um", "uh", "like", "you know"],
    connectors: [ConnectorRow(displayName: "Claude", trigger: "hey claude", enabled: true)],
    buildInfo: "build preview · 2026-07-02 19:41"
)

let encoder = JSONEncoder()
let onboardingJSON = String(data: try encoder.encode(onboarding), encoding: .utf8)!
let settingsJSON = String(data: try encoder.encode(settings), encoding: .utf8)!

onboardingJSON.withCString { _ = azadUIShowOnboarding($0) }
settingsJSON.withCString { _ = azadUIShowSettings($0) }

app.activate(ignoringOtherApps: true)
app.run()

