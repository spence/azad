import AppKit
import Foundation

public typealias AzadUICallback = @convention(c) (UnsafePointer<CChar>?) -> Void

private var eventCallback: AzadUICallback?

@discardableResult
private func onMain(_ body: @escaping () -> Bool) -> Int32 {
    if Thread.isMainThread {
        return body() ? 1 : 0
    }

    var ok = false
    DispatchQueue.main.sync {
        ok = body()
    }
    return ok ? 1 : 0
}

@_cdecl("azad_ui_register_callback")
public func azadUIRegisterCallback(_ callback: AzadUICallback?) {
    eventCallback = callback
}

@_cdecl("azad_ui_show_onboarding")
public func azadUIShowOnboarding(_ json: UnsafePointer<CChar>?) -> Int32 {
    onMain { AzadUI.shared.showOnboarding(json) }
}

@_cdecl("azad_ui_update_onboarding")
public func azadUIUpdateOnboarding(_ json: UnsafePointer<CChar>?) -> Int32 {
    onMain { AzadUI.shared.updateOnboarding(json) }
}

@_cdecl("azad_ui_close_onboarding")
public func azadUICloseOnboarding() -> Int32 {
    onMain {
        AzadUI.shared.closeOnboarding()
        return true
    }
}

@_cdecl("azad_ui_show_settings")
public func azadUIShowSettings(_ json: UnsafePointer<CChar>?) -> Int32 {
    onMain { AzadUI.shared.showSettings(json) }
}

@_cdecl("azad_ui_update_settings")
public func azadUIUpdateSettings(_ json: UnsafePointer<CChar>?) -> Int32 {
    onMain { AzadUI.shared.updateSettings(json) }
}

@_cdecl("azad_ui_refresh_settings_permissions")
public func azadUIRefreshSettingsPermissions(_ json: UnsafePointer<CChar>?) -> Int32 {
    onMain { AzadUI.shared.refreshSettingsPermissions(json) }
}

@_cdecl("azad_ui_sync_listen_modifiers")
public func azadUISyncListenModifiers(_ mask: UInt8) -> Int32 {
    onMain {
        AzadUI.shared.syncListenModifiers(mask)
        return true
    }
}

public final class AzadUI: NSObject {
    public static let shared = AzadUI()

    private let decoder = JSONDecoder()
    private let encoder = JSONEncoder()
    private var onboardingController: OnboardingWindowController?
    private var settingsController: SettingsWindowController?

    private override init() {
        super.init()
    }

    func showOnboarding(_ json: UnsafePointer<CChar>?) -> Bool {
        guard let model: OnboardingViewModel = decode(json) else { return false }
        let controller = onboardingController ?? OnboardingWindowController()
        onboardingController = controller
        enterForegroundWindowMode(menuMode: .onboarding)
        controller.show(model: model)
        return true
    }

    func updateOnboarding(_ json: UnsafePointer<CChar>?) -> Bool {
        guard let model: OnboardingViewModel = decode(json) else { return false }
        onboardingController?.update(model: model)
        return true
    }

    func closeOnboarding() {
        onboardingController?.close()
        onboardingController = nil
        updateActivationPolicyAfterWindowClose()
    }

    func showSettings(_ json: UnsafePointer<CChar>?) -> Bool {
        guard let model: SettingsViewModel = decode(json) else { return false }
        let controller = settingsController ?? SettingsWindowController()
        settingsController = controller
        enterForegroundWindowMode(menuMode: .settings)
        controller.show(model: model)
        return true
    }

    func updateSettings(_ json: UnsafePointer<CChar>?) -> Bool {
        guard let model: SettingsViewModel = decode(json) else { return false }
        settingsController?.update(model: model)
        return true
    }

    func refreshSettingsPermissions(_ json: UnsafePointer<CChar>?) -> Bool {
        guard let update: SettingsPermissionUpdate = decode(json) else { return false }
        settingsController?.updatePermissions(
            accessibility: update.accessibilityStatus,
            microphone: update.microphoneStatus
        )
        return true
    }

    func syncListenModifiers(_ mask: UInt8) {
        onboardingController?.syncListenModifiers(mask)
        settingsController?.syncListenModifiers(mask)
    }

    func updateActivationPolicyAfterWindowClose() {
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            let hasVisibleWindow =
                self.onboardingController?.window?.isVisible == true ||
                self.settingsController?.window?.isVisible == true
            if !hasVisibleWindow {
                NSApp.setActivationPolicy(.accessory)
            }
        }
    }

    private enum AppMenuMode {
        case onboarding
        case settings
    }

    private func enterForegroundWindowMode(menuMode: AppMenuMode) {
        NSApp.setActivationPolicy(.regular)
        NSApp.mainMenu = makeAppMenu(mode: menuMode)
        NSApp.activate(ignoringOtherApps: true)
    }

    private func makeAppMenu(mode: AppMenuMode) -> NSMenu {
        let mainMenu = NSMenu()
        let appItem = NSMenuItem()
        mainMenu.addItem(appItem)

        let appMenu = NSMenu(title: "Azad")
        appItem.submenu = appMenu

        switch mode {
        case .onboarding:
            let quitItem = NSMenuItem(
                title: "Quit Azad",
                action: #selector(NSApplication.terminate(_:)),
                keyEquivalent: "q"
            )
            quitItem.target = NSApp
            appMenu.addItem(quitItem)
        case .settings:
            let closeItem = NSMenuItem(
                title: "Close Settings",
                action: #selector(closeSettingsFromMenu(_:)),
                keyEquivalent: "q"
            )
            closeItem.target = self
            appMenu.addItem(closeItem)
        }

        return mainMenu
    }

    @objc private func closeSettingsFromMenu(_ sender: Any?) {
        settingsController?.close()
        settingsController = nil
        updateActivationPolicyAfterWindowClose()
    }

    func emit(_ event: UIEvent) {
        guard let data = try? encoder.encode(event),
              let text = String(data: data, encoding: .utf8)
        else {
            return
        }
        text.withCString { ptr in
            eventCallback?(ptr)
        }
    }

    private func decode<T: Decodable>(_ json: UnsafePointer<CChar>?) -> T? {
        guard let json else { return nil }
        let text = String(cString: json)
        guard let data = text.data(using: .utf8) else { return nil }
        do {
            return try decoder.decode(T.self, from: data)
        } catch {
            fputs("AzadUI decode error: \(error)\n", stderr)
            return nil
        }
    }
}
