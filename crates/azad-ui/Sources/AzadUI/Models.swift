import AppKit
import Foundation

public enum SettingsTab: String, Codable, CaseIterable {
    case general
    case text
    case models
    case permissions
    case debug
    case connectors
}

public enum PermissionStatus: String, Codable {
    case granted
    case denied
    case notDetermined
    case notGranted
    case unknown
}

extension PermissionStatus {
    var isGranted: Bool {
        self == .granted
    }

    var statusText: String {
        switch self {
        case .granted:
            "Granted"
        case .unknown:
            "Unknown"
        case .denied, .notDetermined, .notGranted:
            "Not granted"
        }
    }

    var statusIconName: String {
        isGranted ? "checkmark.circle.fill" : "circle.fill"
    }

    var statusColor: NSColor {
        isGranted ? Design.green : Design.orange
    }

    var actionTitle: String {
        self == .notDetermined ? "Request" : "Open Settings"
    }

    var requestsPermission: Bool {
        self == .notDetermined
    }
}

public enum ModelStatus: String, Codable {
    case notDownloaded
    case downloading
    case resumable
    case ready
    case failed
}

public struct ModelPack: Codable {
    public let id: String
    public let welcomeName: String
    public let settingsName: String
    public let pageUrl: String
    public let description: String
    public let sizeLabel: String
    public let status: ModelStatus
    public let downloadPaused: Bool
    public let progressPct: UInt8
    public let bytesDoneLabel: String
    public let bytesTotalLabel: String
    public let errorMessage: String

    public init(
        id: String,
        welcomeName: String,
        settingsName: String,
        pageUrl: String,
        description: String,
        sizeLabel: String,
        status: ModelStatus,
        downloadPaused: Bool,
        progressPct: UInt8,
        bytesDoneLabel: String,
        bytesTotalLabel: String,
        errorMessage: String
    ) {
        self.id = id
        self.welcomeName = welcomeName
        self.settingsName = settingsName
        self.pageUrl = pageUrl
        self.description = description
        self.sizeLabel = sizeLabel
        self.status = status
        self.downloadPaused = downloadPaused
        self.progressPct = progressPct
        self.bytesDoneLabel = bytesDoneLabel
        self.bytesTotalLabel = bytesTotalLabel
        self.errorMessage = errorMessage
    }
}

public struct DeviceOption: Codable {
    public let id: String
    public let label: String

    public init(id: String, label: String) {
        self.id = id
        self.label = label
    }
}

public struct OnboardingViewModel: Codable {
    public let alwaysListeningEnabled: Bool
    public let overlayPositionIndex: Int
    public let runOnStartupEnabled: Bool
    public let accessibilityStatus: PermissionStatus
    public let microphoneStatus: PermissionStatus
    public let model: ModelPack
    public let getStartedEnabled: Bool
    public let devices: [DeviceOption]
    public let selectedDeviceIndex: Int?
    public let listenModifiers: UInt8

    public init(
        alwaysListeningEnabled: Bool,
        overlayPositionIndex: Int,
        runOnStartupEnabled: Bool,
        accessibilityStatus: PermissionStatus,
        microphoneStatus: PermissionStatus,
        model: ModelPack,
        getStartedEnabled: Bool,
        devices: [DeviceOption],
        selectedDeviceIndex: Int?,
        listenModifiers: UInt8
    ) {
        self.alwaysListeningEnabled = alwaysListeningEnabled
        self.overlayPositionIndex = overlayPositionIndex
        self.runOnStartupEnabled = runOnStartupEnabled
        self.accessibilityStatus = accessibilityStatus
        self.microphoneStatus = microphoneStatus
        self.model = model
        self.getStartedEnabled = getStartedEnabled
        self.devices = devices
        self.selectedDeviceIndex = selectedDeviceIndex
        self.listenModifiers = listenModifiers
    }
}

public struct ConnectorRow: Codable {
    public let displayName: String
    public let trigger: String
    public let enabled: Bool

    public init(displayName: String, trigger: String, enabled: Bool) {
        self.displayName = displayName
        self.trigger = trigger
        self.enabled = enabled
    }
}

public struct SettingsViewModel: Codable {
    public let selectedTab: SettingsTab
    public let accessibilityStatus: PermissionStatus
    public let microphoneStatus: PermissionStatus
    public let runOnStartupEnabled: Bool
    public let startupListenModeIndex: Int
    public let activationLevel: Int
    public let historyEnabled: Bool
    public let pasteMethodIndex: Int
    public let autoSubmitIndex: Int
    public let overlayPositionIndex: Int
    public let appendTrailingSpaceOnPaste: Bool
    public let deduplicateWordsOnPaste: Bool
    public let convertNumberWordsOnPaste: Bool
    public let convertSpokenEmojiOnPaste: Bool
    public let lowercaseExceptUppercaseWordsOnPaste: Bool
    public let removeHesitationsOnPaste: Bool
    public let listenModifiers: UInt8
    public let debugStatsEnabled: Bool
    public let metricsText: String
    public let model: ModelPack
    public let removedWords: [String]
    public let connectors: [ConnectorRow]
    public let buildInfo: String

    public init(
        selectedTab: SettingsTab,
        accessibilityStatus: PermissionStatus,
        microphoneStatus: PermissionStatus,
        runOnStartupEnabled: Bool,
        startupListenModeIndex: Int,
        activationLevel: Int,
        historyEnabled: Bool,
        pasteMethodIndex: Int,
        autoSubmitIndex: Int,
        overlayPositionIndex: Int,
        appendTrailingSpaceOnPaste: Bool,
        deduplicateWordsOnPaste: Bool,
        convertNumberWordsOnPaste: Bool,
        convertSpokenEmojiOnPaste: Bool,
        lowercaseExceptUppercaseWordsOnPaste: Bool,
        removeHesitationsOnPaste: Bool,
        listenModifiers: UInt8,
        debugStatsEnabled: Bool,
        metricsText: String,
        model: ModelPack,
        removedWords: [String],
        connectors: [ConnectorRow],
        buildInfo: String
    ) {
        self.selectedTab = selectedTab
        self.accessibilityStatus = accessibilityStatus
        self.microphoneStatus = microphoneStatus
        self.runOnStartupEnabled = runOnStartupEnabled
        self.startupListenModeIndex = startupListenModeIndex
        self.activationLevel = activationLevel
        self.historyEnabled = historyEnabled
        self.pasteMethodIndex = pasteMethodIndex
        self.autoSubmitIndex = autoSubmitIndex
        self.overlayPositionIndex = overlayPositionIndex
        self.appendTrailingSpaceOnPaste = appendTrailingSpaceOnPaste
        self.deduplicateWordsOnPaste = deduplicateWordsOnPaste
        self.convertNumberWordsOnPaste = convertNumberWordsOnPaste
        self.convertSpokenEmojiOnPaste = convertSpokenEmojiOnPaste
        self.lowercaseExceptUppercaseWordsOnPaste = lowercaseExceptUppercaseWordsOnPaste
        self.removeHesitationsOnPaste = removeHesitationsOnPaste
        self.listenModifiers = listenModifiers
        self.debugStatsEnabled = debugStatsEnabled
        self.metricsText = metricsText
        self.model = model
        self.removedWords = removedWords
        self.connectors = connectors
        self.buildInfo = buildInfo
    }
}

public struct SettingsPermissionUpdate: Codable {
    public let accessibilityStatus: PermissionStatus
    public let microphoneStatus: PermissionStatus
}

struct UIEvent: Codable {
    let surface: String
    let action: String
    var boolValue: Bool?
    var index: Int?
    var bit: UInt8?
    var value: String?
    var packId: String?
    var permission: String?

    init(
        surface: String,
        action: String,
        boolValue: Bool? = nil,
        index: Int? = nil,
        bit: UInt8? = nil,
        value: String? = nil,
        packId: String? = nil,
        permission: String? = nil
    ) {
        self.surface = surface
        self.action = action
        self.boolValue = boolValue
        self.index = index
        self.bit = bit
        self.value = value
        self.packId = packId
        self.permission = permission
    }
}
