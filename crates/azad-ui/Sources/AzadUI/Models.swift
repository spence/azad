import Foundation

public enum SettingsTab: String, Codable, CaseIterable {
    case general
    case models
    case permissions
    case debug
    case connectors
}

public enum PermissionStatus: String, Codable {
    case granted
    case notGranted
    case unknown
}

public enum ModelStatus: String, Codable {
    case notDownloaded
    case downloading
    case ready
    case failed
}

public struct ModelPack: Codable {
    public let id: String
    public let welcomeName: String
    public let settingsName: String
    public let description: String
    public let sizeLabel: String
    public let status: ModelStatus
    public let progressPct: UInt8
    public let bytesDoneLabel: String
    public let bytesTotalLabel: String
    public let errorMessage: String

    public init(
        id: String,
        welcomeName: String,
        settingsName: String,
        description: String,
        sizeLabel: String,
        status: ModelStatus,
        progressPct: UInt8,
        bytesDoneLabel: String,
        bytesTotalLabel: String,
        errorMessage: String
    ) {
        self.id = id
        self.welcomeName = welcomeName
        self.settingsName = settingsName
        self.description = description
        self.sizeLabel = sizeLabel
        self.status = status
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
    public let historyEnabled: Bool
    public let pasteMethodIndex: Int
    public let appendTrailingSpaceOnPaste: Bool
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
        historyEnabled: Bool,
        pasteMethodIndex: Int,
        appendTrailingSpaceOnPaste: Bool,
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
        self.historyEnabled = historyEnabled
        self.pasteMethodIndex = pasteMethodIndex
        self.appendTrailingSpaceOnPaste = appendTrailingSpaceOnPaste
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
    public let pasteMethodIndex: Int
    public let autoSubmitIndex: Int
    public let overlayPositionIndex: Int
    public let appendTrailingSpaceOnPaste: Bool
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
        pasteMethodIndex: Int,
        autoSubmitIndex: Int,
        overlayPositionIndex: Int,
        appendTrailingSpaceOnPaste: Bool,
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
        self.pasteMethodIndex = pasteMethodIndex
        self.autoSubmitIndex = autoSubmitIndex
        self.overlayPositionIndex = overlayPositionIndex
        self.appendTrailingSpaceOnPaste = appendTrailingSpaceOnPaste
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
