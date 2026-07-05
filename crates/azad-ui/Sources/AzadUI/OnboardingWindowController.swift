import AppKit

final class OnboardingWindowController: NSWindowController, NSWindowDelegate {
    private var model: OnboardingViewModel?
    private var shortcutView: ShortcutView?
    private var closingAfterCompletion = false
    private static let windowSize = NSSize(width: 560, height: 370)
    private static let formLabelWidth: CGFloat = 120

    init() {
        let window = NSWindow(
            contentRect: NSRect(origin: .zero, size: Self.windowSize),
            styleMask: [.titled, .closable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        window.title = ""
        window.titleVisibility = .hidden
        window.titlebarAppearsTransparent = true
        window.standardWindowButton(.miniaturizeButton)?.isHidden = true
        window.standardWindowButton(.zoomButton)?.isHidden = true
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

    func closeAfterCompletion() {
        closingAfterCompletion = true
        close()
        closingAfterCompletion = false
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        if closingAfterCompletion {
            return true
        }
        AzadUI.shared.emit(UIEvent(surface: "app", action: "quit"))
        return false
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
            form.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 34),
            form.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -34),
            form.topAnchor.constraint(equalTo: separator.bottomAnchor, constant: 24),
        ])

        let modelRow = ModelRowView(model: model.model, compact: true, target: self, downloadAction: #selector(downloadModel), downloadControlAction: #selector(controlDownload(_:)))
        form.addArrangedSubview(FormRow(label: "Download model", labelWidth: Self.formLabelWidth, control: modelRow))

        let permissionCard = PermissionCard(
            accessibility: model.accessibilityStatus,
            microphone: model.microphoneStatus,
            target: self,
            action: #selector(openPermission(_:)),
            compactGranted: model.accessibilityStatus == .granted && model.microphoneStatus == .granted
        )
        permissionCard.widthAnchor.constraint(equalToConstant: 358).isActive = true
        form.addArrangedSubview(FormRow(label: "Grant permissions", labelWidth: Self.formLabelWidth, control: permissionCard))

        let shortcut = ShortcutView(mask: model.listenModifiers, target: self, action: #selector(toggleModifier(_:)))
        shortcutView = shortcut
        form.addArrangedSubview(FormRow(label: "Listen shortcut", labelWidth: Self.formLabelWidth, control: shortcut))

        let footer = makeFooter(model)
        root.addSubview(footer)
        NSLayoutConstraint.activate([
            footer.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            footer.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            footer.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            footer.heightAnchor.constraint(equalToConstant: 78),
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

        let permissionsMissing = !model.accessibilityStatus.isGranted || !model.microphoneStatus.isGranted
        let modelMissing = model.model.status == .notDownloaded || model.model.status == .failed
        let modelResumable = model.model.status == .resumable

        if model.model.status == .downloading && permissionsMissing {
            return "Grant permissions while the model downloads."
        }
        if modelResumable && permissionsMissing {
            return "Resume the model download and grant permissions to continue."
        }
        if modelResumable {
            return "Resume the model download to continue."
        }
        if modelMissing && permissionsMissing {
            return "Download a model and grant permissions to continue."
        }
        if modelMissing {
            return "Download the model to continue."
        }
        if permissionsMissing {
            return "Grant permissions to continue."
        }
        return "Finish setup to continue."
    }

    @objc private func getStarted() {
        AzadUI.shared.emit(UIEvent(surface: "onboarding", action: "getStarted"))
    }

    @objc private func toggleModifier(_ sender: KeycapButton) {
        AzadUI.shared.emit(UIEvent(surface: "onboarding", action: "setListenModifier", boolValue: sender.state == .on, bit: sender.bit))
    }

    @objc private func downloadModel() {
        AzadUI.shared.emit(UIEvent(surface: "onboarding", action: "downloadModel", packId: model?.model.id))
    }

    @objc private func controlDownload(_ sender: NSButton) {
        let action = sender.tag == 1 ? "resumeDownload" : "pauseDownload"
        AzadUI.shared.emit(UIEvent(surface: "onboarding", action: action))
    }

    @objc private func openPermission(_ sender: NSButton) {
        guard let permission = sender.identifier?.rawValue else { return }
        if sender.tag == permissionRequestButtonTag {
            performNativePermissionRequest(permission) {
                AzadUI.shared.emit(UIEvent(surface: "onboarding", action: "requestPermission", permission: permission))
            }
            return
        }
        AzadUI.shared.emit(UIEvent(surface: "onboarding", action: "openPermission", permission: permission))
    }
}
