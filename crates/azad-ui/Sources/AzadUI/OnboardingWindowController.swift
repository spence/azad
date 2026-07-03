import AppKit

final class OnboardingWindowController: NSWindowController, NSWindowDelegate {
    private var model: OnboardingViewModel?
    private var shortcutView: ShortcutView?

    init() {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 640, height: 640),
            styleMask: [.borderless],
            backing: .buffered,
            defer: false
        )
        window.isOpaque = false
        window.backgroundColor = .clear
        window.hasShadow = true
        window.isMovableByWindowBackground = true
        window.center()
        super.init(window: window)
        window.delegate = self
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func show(model: OnboardingViewModel) {
        self.model = model
        render(model)
        NSApp.activate(ignoringOtherApps: true)
        window?.center()
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
    }

    func update(model: OnboardingViewModel) {
        self.model = model
        guard window?.isVisible == true else { return }
        render(model)
    }

    func syncListenModifiers(_ mask: UInt8) {
        shortcutView?.sync(mask: mask)
    }

    func windowWillClose(_ notification: Notification) {
        AzadUI.shared.updateActivationPolicyAfterWindowClose()
    }

    private func render(_ model: OnboardingViewModel) {
        let root = ThemedLayerView(fill: Design.window, radius: 12)
        root.layer?.cornerRadius = 12
        window?.contentView = root

        let header = makeHeader()
        root.addSubview(header)
        NSLayoutConstraint.activate([
            header.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            header.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            header.topAnchor.constraint(equalTo: root.topAnchor),
            header.heightAnchor.constraint(equalToConstant: 74),
        ])

        let separator = Design.separatorView()
        root.addSubview(separator)
        NSLayoutConstraint.activate([
            separator.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            separator.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            separator.topAnchor.constraint(equalTo: header.bottomAnchor),
        ])

        let form = NSStackView()
        form.orientation = .vertical
        form.alignment = .leading
        form.spacing = 12
        form.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(form)
        NSLayoutConstraint.activate([
            form.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 68),
            form.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -50),
            form.topAnchor.constraint(equalTo: separator.bottomAnchor, constant: 26),
        ])

        let modelRow = ModelRowView(model: model.model, compact: true, target: self, downloadAction: #selector(downloadModel), cancelAction: #selector(cancelDownload))
        form.addArrangedSubview(FormRow(label: "Model", control: modelRow))

        let startListening = Design.popup(["Automatically", "Manually"], selected: model.alwaysListeningEnabled ? 0 : 1, target: self, action: #selector(setTrigger(_:)))
        startListening.widthAnchor.constraint(equalToConstant: 248).isActive = true
        form.addArrangedSubview(FormRow(label: "Start listening", control: startListening))

        let shortcut = ShortcutView(mask: model.listenModifiers, target: self, action: #selector(toggleModifier(_:)))
        shortcutView = shortcut
        form.addArrangedSubview(FormRow(label: "Listen shortcut", control: shortcut))

        form.addArrangedSubview(FormRow(label: "History", control: Design.checkbox("Keep a searchable history of dictations", checked: model.historyEnabled, target: self, action: #selector(toggleHistory(_:)))))

        let insertPopup = Design.popup(["Paste", "Type", "Type and copy"], selected: model.pasteMethodIndex, target: self, action: nil)
        insertPopup.isEnabled = false
        insertPopup.widthAnchor.constraint(equalToConstant: 182).isActive = true
        form.addArrangedSubview(FormRow(label: "Insert text by", control: insertPopup))

        form.addArrangedSubview(FormRow(label: "Trailing space", control: Design.checkbox("Append a space after each insert", checked: model.appendTrailingSpaceOnPaste, target: self, action: #selector(toggleTrailingSpace(_:)))))

        let overlayPopup = Design.popup(["Follow cursor", "Primary monitor", "Active window"], selected: model.overlayPositionIndex, target: self, action: #selector(setOverlayPosition(_:)))
        overlayPopup.widthAnchor.constraint(equalToConstant: 212).isActive = true
        form.addArrangedSubview(FormRow(label: "Overlay position", control: overlayPopup))

        form.addArrangedSubview(FormRow(label: "Startup", control: Design.checkbox("Open Azad automatically at login", checked: model.runOnStartupEnabled, target: self, action: #selector(toggleLogin(_:)))))

        let permissionCard = PermissionCard(
            accessibility: model.accessibilityStatus,
            microphone: model.microphoneStatus,
            target: self,
            action: #selector(openPermission(_:)),
            compactGranted: model.accessibilityStatus == .granted && model.microphoneStatus == .granted
        )
        permissionCard.widthAnchor.constraint(equalToConstant: 410).isActive = true
        form.addArrangedSubview(FormRow(label: "Permissions", control: permissionCard))

        let devicePopup = Design.popup(model.devices.map(\.label), selected: model.selectedDeviceIndex ?? 0, target: self, action: #selector(selectDevice(_:)))
        devicePopup.widthAnchor.constraint(equalToConstant: 280).isActive = true
        devicePopup.isEnabled = !model.devices.isEmpty
        form.addArrangedSubview(FormRow(label: "Microphone device", control: devicePopup))

        let footer = makeFooter(model)
        root.addSubview(footer)
        NSLayoutConstraint.activate([
            footer.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            footer.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            footer.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            footer.heightAnchor.constraint(equalToConstant: 70),
        ])
    }

    private func makeHeader() -> NSView {
        let view = ThemedLayerView(fill: Design.windowChrome)

        let row = NSStackView()
        row.orientation = .horizontal
        row.alignment = .centerY
        row.spacing = 12
        row.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(row)

        row.addArrangedSubview(Design.appIconView())

        let text = NSStackView()
        text.orientation = .vertical
        text.alignment = .leading
        text.spacing = 1
        text.addArrangedSubview(Design.label("Welcome to Azad", size: 16, weight: .semibold))
        text.addArrangedSubview(Design.label("Finish setup to start dictating.", size: 12, color: Design.secondaryText))
        row.addArrangedSubview(text)

        NSLayoutConstraint.activate([
            row.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            row.centerYAnchor.constraint(equalTo: view.centerYAnchor),
        ])
        return view
    }

    private func makeFooter(_ model: OnboardingViewModel) -> NSView {
        let view = ThemedLayerView(fill: Design.windowChrome)

        let separator = Design.separatorView()
        view.addSubview(separator)
        NSLayoutConstraint.activate([
            separator.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            separator.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            separator.topAnchor.constraint(equalTo: view.topAnchor),
        ])

        let button = Design.primaryButton("Get started", target: self, action: #selector(getStarted))
        button.isEnabled = model.getStartedEnabled
        view.addSubview(button)
        NSLayoutConstraint.activate([
            button.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            button.topAnchor.constraint(equalTo: view.topAnchor, constant: 10),
            button.widthAnchor.constraint(equalToConstant: 150),
        ])

        let hint = Design.label(footerHint(model), size: 12, color: Design.mutedText)
        hint.alignment = .center
        view.addSubview(hint)
        NSLayoutConstraint.activate([
            hint.leadingAnchor.constraint(equalTo: view.leadingAnchor, constant: 20),
            hint.trailingAnchor.constraint(equalTo: view.trailingAnchor, constant: -20),
            hint.topAnchor.constraint(equalTo: button.bottomAnchor, constant: 6),
            hint.bottomAnchor.constraint(lessThanOrEqualTo: view.bottomAnchor, constant: -6),
        ])
        return view
    }

    private func footerHint(_ model: OnboardingViewModel) -> String {
        if model.getStartedEnabled {
            return "Ready to start dictating."
        }
        if model.model.status == .notDownloaded || model.model.status == .failed {
            return "Download a model and grant permissions to continue."
        }
        return "Grant permissions to continue."
    }

    @objc private func getStarted() {
        AzadUI.shared.emit(UIEvent(surface: "onboarding", action: "getStarted"))
    }

    @objc private func setTrigger(_ sender: NSPopUpButton) {
        AzadUI.shared.emit(UIEvent(surface: "onboarding", action: "setTrigger", index: sender.indexOfSelectedItem))
    }

    @objc private func toggleHistory(_ sender: NSButton) {
        AzadUI.shared.emit(UIEvent(surface: "onboarding", action: "toggleHistory", boolValue: sender.state == .on))
    }

    @objc private func toggleTrailingSpace(_ sender: NSButton) {
        AzadUI.shared.emit(UIEvent(surface: "onboarding", action: "toggleAppendTrailingSpace", boolValue: sender.state == .on))
    }

    @objc private func setOverlayPosition(_ sender: NSPopUpButton) {
        AzadUI.shared.emit(UIEvent(surface: "onboarding", action: "setOverlayPosition", index: sender.indexOfSelectedItem))
    }

    @objc private func toggleLogin(_ sender: NSButton) {
        AzadUI.shared.emit(UIEvent(surface: "onboarding", action: "toggleLogin", boolValue: sender.state == .on))
    }

    @objc private func selectDevice(_ sender: NSPopUpButton) {
        AzadUI.shared.emit(UIEvent(surface: "onboarding", action: "selectDevice", index: sender.indexOfSelectedItem))
    }

    @objc private func toggleModifier(_ sender: KeycapButton) {
        AzadUI.shared.emit(UIEvent(surface: "onboarding", action: "setListenModifier", boolValue: sender.state == .on, bit: sender.bit))
    }

    @objc private func downloadModel() {
        AzadUI.shared.emit(UIEvent(surface: "onboarding", action: "downloadModel", packId: model?.model.id))
    }

    @objc private func cancelDownload() {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "cancelDownload"))
    }

    @objc private func openPermission(_ sender: NSButton) {
        AzadUI.shared.emit(UIEvent(surface: "onboarding", action: "openPermission", permission: sender.identifier?.rawValue))
    }
}
