import AppKit

final class SettingsWindowController: NSWindowController, NSWindowDelegate {
    private var model: SettingsViewModel?
    private var selectedTab: SettingsTab = .general
    private let sidebar = NSStackView()
    private let content = NSView()
    private var shortcutView: ShortcutView?

    init() {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 656, height: 391),
            styleMask: [.titled, .closable, .miniaturizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Azad Settings"
        window.center()
        super.init(window: window)
        window.delegate = self
        configureRoot()
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func show(model: SettingsViewModel) {
        self.model = model
        selectedTab = model.selectedTab
        render()
        NSApp.activate(ignoringOtherApps: true)
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
        DispatchQueue.main.async { [weak self] in
            self?.window?.makeFirstResponder(nil)
        }
    }

    func update(model: SettingsViewModel) {
        self.model = model
        guard window?.isVisible == true else { return }
        render()
    }

    func updatePermissions(accessibility: PermissionStatus, microphone: PermissionStatus) {
        guard let model else { return }
        if model.accessibilityStatus == accessibility && model.microphoneStatus == microphone {
            return
        }
        self.model = SettingsViewModel(
            selectedTab: model.selectedTab,
            accessibilityStatus: accessibility,
            microphoneStatus: microphone,
            runOnStartupEnabled: model.runOnStartupEnabled,
            pasteMethodIndex: model.pasteMethodIndex,
            autoSubmitIndex: model.autoSubmitIndex,
            overlayPositionIndex: model.overlayPositionIndex,
            appendTrailingSpaceOnPaste: model.appendTrailingSpaceOnPaste,
            deduplicateWordsOnPaste: model.deduplicateWordsOnPaste,
            listenModifiers: model.listenModifiers,
            debugStatsEnabled: model.debugStatsEnabled,
            metricsText: model.metricsText,
            model: model.model,
            removedWords: model.removedWords,
            connectors: model.connectors,
            buildInfo: model.buildInfo
        )
        guard window?.isVisible == true else { return }
        render()
    }

    func syncListenModifiers(_ mask: UInt8) {
        shortcutView?.sync(mask: mask)
    }

    func windowWillClose(_ notification: Notification) {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "windowClosed"))
        NSApp.setActivationPolicy(.accessory)
    }

    private func configureRoot() {
        guard let window else { return }
        let root = NSView()
        root.wantsLayer = true
        root.layer?.backgroundColor = Design.window.cgColor
        root.translatesAutoresizingMaskIntoConstraints = false
        window.contentView = root

        sidebar.orientation = .vertical
        sidebar.alignment = .leading
        sidebar.spacing = 4
        sidebar.edgeInsets = NSEdgeInsets(top: 16, left: 12, bottom: 16, right: 14)
        sidebar.translatesAutoresizingMaskIntoConstraints = false
        sidebar.wantsLayer = true
        sidebar.layer?.backgroundColor = Design.panel.cgColor
        root.addSubview(sidebar)

        content.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(content)

        let separator = NSView()
        separator.wantsLayer = true
        separator.layer?.backgroundColor = Design.separator.cgColor
        separator.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(separator)

        NSLayoutConstraint.activate([
            sidebar.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            sidebar.topAnchor.constraint(equalTo: root.topAnchor),
            sidebar.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            sidebar.widthAnchor.constraint(equalToConstant: 158),

            separator.leadingAnchor.constraint(equalTo: sidebar.trailingAnchor),
            separator.topAnchor.constraint(equalTo: root.topAnchor),
            separator.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            separator.widthAnchor.constraint(equalToConstant: 1),

            content.leadingAnchor.constraint(equalTo: separator.trailingAnchor),
            content.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            content.topAnchor.constraint(equalTo: root.topAnchor),
            content.bottomAnchor.constraint(equalTo: root.bottomAnchor),
        ])
    }

    private func render() {
        guard let model else { return }
        renderSidebar()
        content.subviews.forEach { $0.removeFromSuperview() }
        shortcutView = nil

        let pane: NSView
        switch selectedTab {
        case .general:
            pane = generalPane(model)
        case .models:
            pane = modelsPane(model)
        case .permissions:
            pane = permissionsPane(model)
        case .debug:
            pane = debugPane(model)
        case .connectors:
            pane = connectorsPane(model)
        }
        content.addSubview(pane)
        pane.pinToSuperview(NSEdgeInsets(top: 24, left: 32, bottom: 34, right: 24))

        let build = Design.label(model.buildInfo, size: 10, color: Design.mutedText)
        build.alignment = .right
        content.addSubview(build)
        NSLayoutConstraint.activate([
            build.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -16),
            build.bottomAnchor.constraint(equalTo: content.bottomAnchor, constant: -10),
        ])
    }

    private func renderSidebar() {
        sidebar.arrangedSubviews.forEach {
            sidebar.removeArrangedSubview($0)
            $0.removeFromSuperview()
        }
        let rows: [(SettingsTab, String, String)] = [
            (.general, "sun.max", "General"),
            (.models, "arrow.down.circle", "Models"),
            (.permissions, "lock", "Permissions"),
            (.connectors, "link", "Connectors"),
            (.debug, "ladybug", "Debug"),
        ]
        for row in rows {
            let button = SidebarButton(tab: row.0, icon: row.1, title: row.2, selected: row.0 == selectedTab, target: self, action: #selector(selectTab(_:)))
            sidebar.addArrangedSubview(button)
        }
    }

    private func generalPane(_ model: SettingsViewModel) -> NSView {
        let stack = paneStack()

        stack.addArrangedSubview(FormRow(label: "Startup", control: Design.checkbox("Run Azad on startup", checked: model.runOnStartupEnabled, target: self, action: #selector(toggleRunOnStartup(_:)))))

        let paste = Design.popup(["Paste", "Type", "Type and copy"], selected: model.pasteMethodIndex, target: self, action: #selector(selectPasteMethod(_:)))
        paste.widthAnchor.constraint(equalToConstant: 210).isActive = true
        stack.addArrangedSubview(FormRow(label: "Insert method", control: paste))

        let submit = Design.popup(["Off", "Enter", "Ctrl+Enter", "Shift+Enter"], selected: model.autoSubmitIndex, target: self, action: #selector(selectAutoSubmit(_:)))
        submit.widthAnchor.constraint(equalToConstant: 170).isActive = true
        stack.addArrangedSubview(FormRow(label: "Auto submit", control: submit))

        let overlay = Design.popup(["Follow cursor", "Primary monitor", "Active window"], selected: model.overlayPositionIndex, target: self, action: #selector(selectOverlayPosition(_:)))
        overlay.widthAnchor.constraint(equalToConstant: 220).isActive = true
        stack.addArrangedSubview(FormRow(label: "Overlay position", control: overlay))

        let shortcut = ShortcutView(mask: model.listenModifiers, target: self, action: #selector(toggleModifier(_:)))
        shortcutView = shortcut
        stack.addArrangedSubview(FormRow(label: "Listen shortcut", control: shortcut))

        stack.addArrangedSubview(FormRow(label: "Trailing space", control: Design.checkbox("Append trailing space after paste", checked: model.appendTrailingSpaceOnPaste, target: self, action: #selector(toggleTrailingSpace(_:)))))
        stack.addArrangedSubview(FormRow(label: "Repeated words", control: Design.checkbox("Collapse adjacent duplicate words", checked: model.deduplicateWordsOnPaste, target: self, action: #selector(toggleDeduplicateWords(_:)))))

        stack.addArrangedSubview(removedWordsRow(model))
        return stack
    }

    private func removedWordsRow(_ model: SettingsViewModel) -> NSView {
        let row = NSView()
        row.translatesAutoresizingMaskIntoConstraints = false

        let label = Design.label("Removed words", size: 13, color: Design.secondaryText)
        label.alignment = .right
        row.addSubview(label)

        let addRow = NSStackView()
        addRow.orientation = .horizontal
        addRow.alignment = .centerY
        addRow.spacing = 10
        addRow.translatesAutoresizingMaskIntoConstraints = false
        row.addSubview(addRow)

        let field = NSTextField()
        field.placeholderString = "Enter word"
        field.controlSize = .large
        field.translatesAutoresizingMaskIntoConstraints = false
        field.widthAnchor.constraint(equalToConstant: 178).isActive = true
        field.identifier = NSUserInterfaceItemIdentifier("removed-word-input")
        addRow.addArrangedSubview(field)

        let add = Design.pushButton("Add", target: self, action: #selector(addWord(_:)))
        add.widthAnchor.constraint(equalToConstant: 70).isActive = true
        addRow.addArrangedSubview(add)

        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: row.leadingAnchor),
            label.widthAnchor.constraint(equalToConstant: 150),
            label.centerYAnchor.constraint(equalTo: addRow.centerYAnchor),
            addRow.leadingAnchor.constraint(equalTo: label.trailingAnchor, constant: 14),
            addRow.topAnchor.constraint(equalTo: row.topAnchor),
            addRow.trailingAnchor.constraint(equalTo: row.trailingAnchor),
        ])

        if model.removedWords.isEmpty {
            addRow.bottomAnchor.constraint(equalTo: row.bottomAnchor).isActive = true
        } else {
            let chips = NSStackView()
            chips.orientation = .horizontal
            chips.alignment = .centerY
            chips.spacing = 6
            chips.translatesAutoresizingMaskIntoConstraints = false
            for word in model.removedWords {
                chips.addArrangedSubview(WordChip(word: word, target: self, action: #selector(removeWord(_:))))
            }
            row.addSubview(chips)
            NSLayoutConstraint.activate([
                chips.leadingAnchor.constraint(equalTo: addRow.leadingAnchor),
                chips.topAnchor.constraint(equalTo: addRow.bottomAnchor, constant: 8),
                chips.trailingAnchor.constraint(lessThanOrEqualTo: row.trailingAnchor),
                chips.bottomAnchor.constraint(equalTo: row.bottomAnchor),
            ])
        }

        return row
    }

    private func modelsPane(_ model: SettingsViewModel) -> NSView {
        let stack = paneStack()
        let title = Design.label("\(model.model.settingsName) ↗", size: 15, weight: .semibold, color: Design.blue)
        stack.addArrangedSubview(title)
        stack.addArrangedSubview(Design.label(model.model.description, size: 13, color: Design.secondaryText))
        let row = ModelRowView(model: model.model, compact: false, target: self, downloadAction: #selector(downloadModel), cancelAction: #selector(cancelDownload))
        stack.addArrangedSubview(row)
        row.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        return stack
    }

    private func permissionsPane(_ model: SettingsViewModel) -> NSView {
        let root = NSView()
        root.translatesAutoresizingMaskIntoConstraints = false

        let card = PermissionCard(
            accessibility: model.accessibilityStatus,
            microphone: model.microphoneStatus,
            target: self,
            action: #selector(openPermission(_:)),
            compactGranted: false,
            showMissingHint: false,
            framed: false
        )
        root.addSubview(card)

        let hint = Design.label("Required to capture audio and insert text. Click Open Settings to grant.", size: 12, color: Design.mutedText)
        root.addSubview(hint)

        NSLayoutConstraint.activate([
            card.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            card.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            card.topAnchor.constraint(equalTo: root.topAnchor),
            card.heightAnchor.constraint(equalToConstant: 72),
            hint.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            hint.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            hint.topAnchor.constraint(equalTo: card.bottomAnchor, constant: 12),
        ])
        return root
    }

    private func debugPane(_ model: SettingsViewModel) -> NSView {
        let root = NSView()
        root.translatesAutoresizingMaskIntoConstraints = false

        let row = NSStackView()
        row.orientation = .horizontal
        row.alignment = .centerY
        row.spacing = 10
        row.translatesAutoresizingMaskIntoConstraints = false
        row.addArrangedSubview(Design.checkbox("Enable debug statistics", checked: model.debugStatsEnabled, target: self, action: #selector(toggleDebugStats(_:))))
        row.addArrangedSubview(NSView())
        let refresh = Design.pushButton("Refresh", target: self, action: #selector(refresh))
        refresh.widthAnchor.constraint(equalToConstant: 88).isActive = true
        row.addArrangedSubview(refresh)
        root.addSubview(row)

        let metricsPanel = Design.roundedPanel()
        metricsPanel.layer?.backgroundColor = NSColor(calibratedWhite: 0.06, alpha: 0.7).cgColor
        metricsPanel.layer?.masksToBounds = true
        root.addSubview(metricsPanel)

        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.scrollerStyle = .overlay
        scroll.drawsBackground = false
        scroll.borderType = .noBorder
        scroll.translatesAutoresizingMaskIntoConstraints = false
        let text = NSTextView()
        text.isEditable = false
        text.isSelectable = true
        text.drawsBackground = false
        text.textColor = Design.secondaryText
        text.font = .monospacedSystemFont(ofSize: 10, weight: .regular)
        text.string = model.metricsText
        scroll.documentView = text
        metricsPanel.addSubview(scroll)

        NSLayoutConstraint.activate([
            row.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            row.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            row.topAnchor.constraint(equalTo: root.topAnchor),
            row.heightAnchor.constraint(equalToConstant: 32),
            metricsPanel.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            metricsPanel.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            metricsPanel.topAnchor.constraint(equalTo: row.bottomAnchor, constant: 14),
            metricsPanel.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            scroll.leadingAnchor.constraint(equalTo: metricsPanel.leadingAnchor, constant: 10),
            scroll.trailingAnchor.constraint(equalTo: metricsPanel.trailingAnchor, constant: -8),
            scroll.topAnchor.constraint(equalTo: metricsPanel.topAnchor, constant: 8),
            scroll.bottomAnchor.constraint(equalTo: metricsPanel.bottomAnchor, constant: -8),
        ])
        return root
    }

    private func connectorsPane(_ model: SettingsViewModel) -> NSView {
        let stack = paneStack()
        stack.addArrangedSubview(Design.label("Open an utterance with a connector's phrase to tag it.", size: 12, color: Design.mutedText))

        for (index, connector) in model.connectors.enumerated() {
            let row = NSStackView()
            row.orientation = .horizontal
            row.alignment = .centerY
            row.spacing = 10
            row.edgeInsets = NSEdgeInsets(top: 0, left: 14, bottom: 0, right: 14)
            row.translatesAutoresizingMaskIntoConstraints = false
            row.wantsLayer = true
            row.layer?.backgroundColor = Design.panel.cgColor
            row.layer?.borderColor = Design.border.cgColor
            row.layer?.borderWidth = 1
            row.layer?.cornerRadius = 8
            row.heightAnchor.constraint(equalToConstant: 58).isActive = true

            let checkbox = Design.checkbox("", checked: connector.enabled, target: self, action: #selector(toggleConnector(_:)))
            checkbox.state = connector.enabled ? .on : .off
            checkbox.tag = index
            row.addArrangedSubview(checkbox)
            row.addArrangedSubview(connectorLogo())
            row.addArrangedSubview(Design.label(connector.displayName, size: 13, weight: .medium))
            row.addArrangedSubview(NSView())
            row.addArrangedSubview(triggerPill(connector.trigger))
            stack.addArrangedSubview(row)
            row.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        }

        return stack
    }

    private func connectorLogo() -> NSView {
        let container = NSView()
        container.translatesAutoresizingMaskIntoConstraints = false
        container.wantsLayer = true
        container.layer?.backgroundColor = Design.claude.cgColor
        container.layer?.cornerRadius = 7
        container.widthAnchor.constraint(equalToConstant: 28).isActive = true
        container.heightAnchor.constraint(equalToConstant: 28).isActive = true

        let logo = Design.claudeLogoView(size: 18)
        container.addSubview(logo)
        NSLayoutConstraint.activate([
            logo.centerXAnchor.constraint(equalTo: container.centerXAnchor),
            logo.centerYAnchor.constraint(equalTo: container.centerYAnchor),
        ])
        return container
    }

    private func triggerPill(_ text: String) -> NSView {
        let container = NSView()
        container.translatesAutoresizingMaskIntoConstraints = false
        container.wantsLayer = true
        container.layer?.backgroundColor = Design.control.cgColor
        container.layer?.cornerRadius = 6
        container.widthAnchor.constraint(greaterThanOrEqualToConstant: 100).isActive = true
        container.heightAnchor.constraint(equalToConstant: 24).isActive = true

        let label = Design.label(text, size: 12, color: Design.secondaryText)
        label.alignment = .center
        container.addSubview(label)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 10),
            label.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -10),
            label.centerYAnchor.constraint(equalTo: container.centerYAnchor),
        ])
        return container
    }

    private func paneStack() -> NSStackView {
        let stack = NSStackView()
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 14
        stack.translatesAutoresizingMaskIntoConstraints = false
        return stack
    }

    @objc private func selectTab(_ sender: SidebarButton) {
        selectedTab = sender.tab
        render()
    }

    @objc private func toggleRunOnStartup(_ sender: NSButton) {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "toggleRunOnStartup", boolValue: sender.state == .on))
    }

    @objc private func selectPasteMethod(_ sender: NSPopUpButton) {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "selectPasteMethod", index: sender.indexOfSelectedItem))
    }

    @objc private func selectAutoSubmit(_ sender: NSPopUpButton) {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "selectAutoSubmit", index: sender.indexOfSelectedItem))
    }

    @objc private func selectOverlayPosition(_ sender: NSPopUpButton) {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "selectOverlayPosition", index: sender.indexOfSelectedItem))
    }

    @objc private func toggleTrailingSpace(_ sender: NSButton) {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "toggleAppendTrailingSpace", boolValue: sender.state == .on))
    }

    @objc private func toggleDeduplicateWords(_ sender: NSButton) {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "toggleDeduplicateWords", boolValue: sender.state == .on))
    }

    @objc private func toggleModifier(_ sender: KeycapButton) {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "setListenModifier", boolValue: sender.state == .on, bit: sender.bit))
    }

    @objc private func toggleDebugStats(_ sender: NSButton) {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "toggleDebugStats", boolValue: sender.state == .on))
    }

    @objc private func refresh() {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "refresh"))
    }

    @objc private func downloadModel() {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "downloadModel", packId: model?.model.id))
    }

    @objc private func cancelDownload() {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "cancelDownload"))
    }

    @objc private func openPermission(_ sender: NSButton) {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "openPermission", permission: sender.identifier?.rawValue))
    }

    @objc private func toggleConnector(_ sender: NSButton) {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "toggleConnector", boolValue: sender.state == .on, index: sender.tag))
    }

    @objc private func addWord(_ sender: NSButton) {
        guard let field = findSubview(identifier: "removed-word-input", in: content) as? NSTextField else { return }
        let text = field.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        field.stringValue = ""
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "addRemovedWord", value: text))
    }

    @objc private func removeWord(_ sender: NSButton) {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "removeRemovedWord", value: sender.identifier?.rawValue))
    }

    private func findSubview(identifier: String, in root: NSView) -> NSView? {
        if root.identifier?.rawValue == identifier {
            return root
        }
        for child in root.subviews {
            if let found = findSubview(identifier: identifier, in: child) {
                return found
            }
        }
        return nil
    }
}

final class SidebarButton: NSButton {
    let tab: SettingsTab
    private let iconView = PassthroughImageView()
    private let titleLabel = PassthroughTextField(labelWithString: "")

    init(tab: SettingsTab, icon: String, title: String, selected: Bool, target: AnyObject?, action: Selector?) {
        self.tab = tab
        super.init(frame: .zero)
        self.target = target
        self.action = action
        self.isBordered = false
        self.title = ""
        self.translatesAutoresizingMaskIntoConstraints = false
        self.widthAnchor.constraint(equalToConstant: 132).isActive = true
        self.heightAnchor.constraint(equalToConstant: 30).isActive = true
        self.wantsLayer = true
        self.layer?.cornerRadius = 7
        self.layer?.backgroundColor = selected ? Design.blue.cgColor : NSColor.clear.cgColor

        iconView.image = NSImage(systemSymbolName: icon, accessibilityDescription: nil)
        iconView.contentTintColor = selected ? .white : Design.secondaryText
        iconView.translatesAutoresizingMaskIntoConstraints = false
        addSubview(iconView)

        titleLabel.stringValue = title
        titleLabel.font = .systemFont(ofSize: 14, weight: selected ? .semibold : .regular)
        titleLabel.textColor = selected ? .white : Design.secondaryText
        titleLabel.translatesAutoresizingMaskIntoConstraints = false
        addSubview(titleLabel)

        NSLayoutConstraint.activate([
            iconView.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
            iconView.centerYAnchor.constraint(equalTo: centerYAnchor),
            iconView.widthAnchor.constraint(equalToConstant: 18),
            iconView.heightAnchor.constraint(equalToConstant: 18),
            titleLabel.leadingAnchor.constraint(equalTo: iconView.trailingAnchor, constant: 8),
            titleLabel.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -10),
            titleLabel.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func mouseDown(with event: NSEvent) {
        guard isEnabled, let target, let action else { return }
        _ = target.perform(action, with: self)
    }
}

final class PassthroughImageView: NSImageView {
    override func hitTest(_ point: NSPoint) -> NSView? {
        nil
    }
}

final class PassthroughTextField: NSTextField {
    override func hitTest(_ point: NSPoint) -> NSView? {
        nil
    }
}

final class WordChip: NSButton {
    init(word: String, target: AnyObject?, action: Selector?) {
        super.init(frame: .zero)
        self.title = "\(word)  x"
        self.target = target
        self.action = action
        self.identifier = NSUserInterfaceItemIdentifier(word)
        self.isBordered = false
        self.font = .systemFont(ofSize: 12, weight: .medium)
        self.contentTintColor = Design.text
        self.translatesAutoresizingMaskIntoConstraints = false
        self.wantsLayer = true
        self.layer?.backgroundColor = Design.control.cgColor
        self.layer?.cornerRadius = 13
        self.heightAnchor.constraint(equalToConstant: 28).isActive = true
        self.widthAnchor.constraint(greaterThanOrEqualToConstant: 54).isActive = true
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }
}
