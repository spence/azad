import AppKit

final class SettingsWindowController: NSWindowController, NSWindowDelegate {
    private var model: SettingsViewModel?
    private var selectedTab: SettingsTab = .general
    private let sidebar = ThemedStackView(fill: Design.sidebar)
    private let content = NSView()
    private var shortcutView: ShortcutView?
    private weak var activationLevelValueLabel: NSTextField?
    /// Toggles the “What can I say?” list under the Azad connector when Apple Intelligence isn’t ready.
    private var showAzadVoiceCommands = false
    /// Toggles the Spotify command list inside the Spotify connector card.
    private var showSpotifyVoiceCommands = false

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
            startupListenModeIndex: model.startupListenModeIndex,
            activationLevel: model.activationLevel,
            historyEnabled: model.historyEnabled,
            pasteMethodIndex: model.pasteMethodIndex,
            autoSubmitIndex: model.autoSubmitIndex,
            overlayPositionIndex: model.overlayPositionIndex,
            appendTrailingSpaceOnPaste: model.appendTrailingSpaceOnPaste,
            deduplicateWordsOnPaste: model.deduplicateWordsOnPaste,
            convertNumberWordsOnPaste: model.convertNumberWordsOnPaste,
            convertSpokenEmojiOnPaste: model.convertSpokenEmojiOnPaste,
            lowercaseExceptUppercaseWordsOnPaste: model.lowercaseExceptUppercaseWordsOnPaste,
            removeHesitationsOnPaste: model.removeHesitationsOnPaste,
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
        AzadUI.shared.updateActivationPolicyAfterWindowClose()
    }

    private func configureRoot() {
        guard let window else { return }
        let root = ThemedLayerView(fill: Design.content)
        window.contentView = root

        sidebar.orientation = .vertical
        sidebar.alignment = .leading
        sidebar.spacing = 4
        sidebar.edgeInsets = NSEdgeInsets(top: 16, left: 12, bottom: 16, right: 14)
        sidebar.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(sidebar)

        content.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(content)

        let separator = ThemedLayerView(fill: Design.separator)
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
        case .text:
            pane = textPane(model)
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
            (.text, "textformat", "Text"),
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

        let startupListen = Design.popup(["Off", "On", "Restore last state"], selected: model.startupListenModeIndex, target: self, action: #selector(selectStartupListenMode(_:)))
        startupListen.widthAnchor.constraint(equalToConstant: 190).isActive = true
        stack.addArrangedSubview(FormRow(label: "Start listening", control: startupListen))

        let shortcut = ShortcutView(mask: model.listenModifiers, target: self, action: #selector(toggleModifier(_:)))
        shortcutView = shortcut
        stack.addArrangedSubview(FormRow(label: "Listen shortcut", control: shortcut))

        let paste = Design.popup(["Paste", "Type", "Type and copy"], selected: model.pasteMethodIndex, target: self, action: #selector(selectPasteMethod(_:)))
        paste.widthAnchor.constraint(equalToConstant: 210).isActive = true
        stack.addArrangedSubview(FormRow(label: "Insert method", control: paste))

        let overlay = Design.popup(["Follow cursor", "Primary monitor", "Active window"], selected: model.overlayPositionIndex, target: self, action: #selector(selectOverlayPosition(_:)))
        overlay.widthAnchor.constraint(equalToConstant: 220).isActive = true
        stack.addArrangedSubview(FormRow(label: "Overlay position", control: overlay))

        stack.addArrangedSubview(FormRow(label: "History", control: Design.checkbox("Keep a searchable history of dictations", checked: model.historyEnabled, target: self, action: #selector(toggleHistory(_:)))))

        let submit = Design.popup(["Off", "Enter", "Ctrl+Enter", "Shift+Enter"], selected: model.autoSubmitIndex, target: self, action: #selector(selectAutoSubmit(_:)))
        submit.widthAnchor.constraint(equalToConstant: 170).isActive = true
        stack.addArrangedSubview(FormRow(label: "Auto submit", control: submit))

        stack.addArrangedSubview(FormRow(label: "Activation level", control: activationLevelControl(model.activationLevel)))

        stack.addArrangedSubview(FormRow(label: "Open at login", control: Design.checkbox("Run Azad on startup", checked: model.runOnStartupEnabled, target: self, action: #selector(toggleRunOnStartup(_:)))))

        return stack
    }

    private func activationLevelControl(_ value: Int) -> NSView {
        let stack = NSStackView()
        stack.orientation = .horizontal
        stack.alignment = .centerY
        stack.spacing = 8
        stack.translatesAutoresizingMaskIntoConstraints = false

        stack.addArrangedSubview(Design.label("Quiet", size: 12, color: Design.secondaryText))

        let slider = NSSlider(value: Double(value), minValue: 0, maxValue: 100, target: self, action: #selector(setActivationLevel(_:)))
        slider.isContinuous = true
        slider.controlSize = .small
        slider.translatesAutoresizingMaskIntoConstraints = false
        slider.widthAnchor.constraint(equalToConstant: 160).isActive = true
        stack.addArrangedSubview(slider)

        stack.addArrangedSubview(Design.label("Loud", size: 12, color: Design.secondaryText))
        let valueLabel = Design.label(activationLevelLabel(for: value), size: 12, color: Design.secondaryText.withAlphaComponent(0.72))
        valueLabel.font = NSFont.monospacedDigitSystemFont(ofSize: 12, weight: .regular)
        valueLabel.setContentHuggingPriority(.required, for: .horizontal)
        stack.addArrangedSubview(valueLabel)
        activationLevelValueLabel = valueLabel
        return stack
    }

    private func activationLevelLabel(for value: Int) -> String {
        let clamped = max(0, min(100, value))
        let db = -60.0 + (Double(clamped) / 100.0) * 40.0
        return "(\(Int(db.rounded())) dB)"
    }

    private func textPane(_ model: SettingsViewModel) -> NSView {
        let stack = paneStack()

        stack.addArrangedSubview(FormRow(label: "Trailing space", control: Design.checkbox("Append trailing space after paste", checked: model.appendTrailingSpaceOnPaste, target: self, action: #selector(toggleTrailingSpace(_:)))))
        stack.addArrangedSubview(FormRow(label: "Repeated words", control: Design.checkbox("Collapse adjacent duplicate words", checked: model.deduplicateWordsOnPaste, target: self, action: #selector(toggleDeduplicateWords(_:)))))
        stack.addArrangedSubview(FormRow(label: "Numbers", control: Design.checkbox("Convert spoken numbers to digits", checked: model.convertNumberWordsOnPaste, target: self, action: #selector(toggleConvertNumberWords(_:)))))
        stack.addArrangedSubview(FormRow(label: "Emoji", control: Design.checkbox("Convert spoken emoji names", checked: model.convertSpokenEmojiOnPaste, target: self, action: #selector(toggleConvertSpokenEmoji(_:)))))
        stack.addArrangedSubview(FormRow(label: "Casing", control: Design.checkbox("Lowercase everything except uppercase words", checked: model.lowercaseExceptUppercaseWordsOnPaste, target: self, action: #selector(toggleLowercaseExceptUppercaseWords(_:)))))

        stack.addArrangedSubview(removedWordsRow(model))
        return stack
    }

    private func removedWordsRow(_ model: SettingsViewModel) -> NSView {
        let row = NSView()
        row.translatesAutoresizingMaskIntoConstraints = false

        let label = Design.label("Removed words", size: 13, color: Design.secondaryText)
        label.alignment = .right
        row.addSubview(label)

        let contentStack = NSStackView()
        contentStack.orientation = .vertical
        contentStack.alignment = .leading
        contentStack.spacing = 10
        contentStack.translatesAutoresizingMaskIntoConstraints = false
        row.addSubview(contentStack)

        let addRow = NSStackView()
        addRow.orientation = .horizontal
        addRow.alignment = .centerY
        addRow.spacing = 10
        addRow.translatesAutoresizingMaskIntoConstraints = false
        contentStack.addArrangedSubview(addRow)

        let field = NSTextField()
        field.placeholderString = "Enter word"
        field.controlSize = .large
        field.translatesAutoresizingMaskIntoConstraints = false
        field.widthAnchor.constraint(equalToConstant: 178).isActive = true
        field.identifier = NSUserInterfaceItemIdentifier("removed-word-input")
        field.target = self
        field.action = #selector(addWord(_:))
        addRow.addArrangedSubview(field)

        let add = Design.pushButton("Add", target: self, action: #selector(addWord(_:)))
        add.widthAnchor.constraint(equalToConstant: 70).isActive = true
        addRow.addArrangedSubview(add)

        let hesitations = Design.checkbox("Hesitations (um, ah, etc.)", checked: model.removeHesitationsOnPaste, target: self, action: #selector(toggleRemoveHesitations(_:)))
        contentStack.addArrangedSubview(hesitations)

        if !model.removedWords.isEmpty {
            let chips = WrappingChipsView(words: model.removedWords, target: self, action: #selector(removeWord(_:)))
            chips.translatesAutoresizingMaskIntoConstraints = false
            contentStack.addArrangedSubview(chips)
            chips.widthAnchor.constraint(equalTo: contentStack.widthAnchor).isActive = true
        }

        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: row.leadingAnchor),
            label.widthAnchor.constraint(equalToConstant: 150),
            label.centerYAnchor.constraint(equalTo: addRow.centerYAnchor),
            contentStack.leadingAnchor.constraint(equalTo: label.trailingAnchor, constant: 14),
            contentStack.topAnchor.constraint(equalTo: row.topAnchor),
            contentStack.trailingAnchor.constraint(equalTo: row.trailingAnchor),
            contentStack.bottomAnchor.constraint(equalTo: row.bottomAnchor),
        ])

        return row
    }

    private func modelsPane(_ model: SettingsViewModel) -> NSView {
        let stack = paneStack()
        let title = Design.linkLabel("\(model.model.settingsName) ↗", url: model.model.pageUrl, size: 15, weight: .semibold)
        stack.addArrangedSubview(title)
        stack.addArrangedSubview(Design.label(model.model.description, size: 13, color: Design.secondaryText))
        let row = ModelRowView(model: model.model, compact: false, target: self, downloadAction: #selector(downloadModel), downloadControlAction: #selector(controlDownload(_:)))
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

        let hint = Design.label("Required to capture audio and insert text.", size: 12, color: Design.mutedText)
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
        if let themedPanel = metricsPanel as? ThemedLayerView {
            themedPanel.fillColor = Design.metricsPanel
        }
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
        // Don’t let rows stretch to fill leftover pane height.
        stack.distribution = .gravityAreas
        // Stretch every arranged card to the full content width so Claude and Azad match.
        stack.alignment = .width
        stack.addArrangedSubview(Design.label("Open an utterance with a connector's phrase to tag it.", size: 12, color: Design.mutedText))

        for (index, connector) in model.connectors.enumerated() {
            let card: NSView
            if connector.id == "azad" {
                card = azadConnectorCard(connector, index: index)
            } else if connector.id == "spotify" {
                card = spotifyConnectorCard(connector, index: index)
            } else {
                card = compactConnectorRow(connector, index: index)
            }
            // Low horizontal hugging so width is driven by the pane, not content intrinsic size.
            card.setContentHuggingPriority(.defaultLow, for: .horizontal)
            card.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
            stack.addArrangedSubview(card)
            card.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        }

        return stack
    }

    /// Claude-style fixed 58pt row. Uses a plain container so logo/pill never stretch
    /// to the card height (NSStackView `.fill` was blowing them up). Full pane width.
    private func compactConnectorRow(_ connector: ConnectorRow, index: Int) -> NSView {
        let card = ThemedLayerView(fill: Design.panel, stroke: Design.border, radius: 8, borderWidth: 1)
        card.layer?.cornerRadius = 8
        card.setContentHuggingPriority(.required, for: .vertical)
        card.setContentCompressionResistancePriority(.required, for: .vertical)
        card.setContentHuggingPriority(.defaultLow, for: .horizontal)
        card.heightAnchor.constraint(equalToConstant: 58).isActive = true

        let row = NSStackView()
        row.orientation = .horizontal
        row.alignment = .centerY
        row.distribution = .fill
        row.spacing = 10
        row.translatesAutoresizingMaskIntoConstraints = false

        let checkbox = Design.checkbox("", checked: connector.enabled, target: self, action: #selector(toggleConnector(_:)))
        checkbox.state = connector.enabled ? .on : .off
        checkbox.isEnabled = connector.canEnable
        checkbox.tag = index
        pinIntrinsicVertical(checkbox)

        let logo = connectorLogo(for: connector)
        pinIntrinsicVertical(logo)

        let name = Design.label(connector.displayName, size: 13, weight: .medium)
        pinIntrinsicVertical(name)

        let spacer = NSView()
        spacer.translatesAutoresizingMaskIntoConstraints = false
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        spacer.setContentHuggingPriority(.required, for: .vertical)
        spacer.heightAnchor.constraint(equalToConstant: 1).isActive = true

        let pill = triggerPill(connector.trigger)
        pinIntrinsicVertical(pill)

        for v in [checkbox, logo, name, spacer, pill] {
            row.addArrangedSubview(v)
        }

        card.addSubview(row)
        NSLayoutConstraint.activate([
            row.leadingAnchor.constraint(equalTo: card.leadingAnchor, constant: 14),
            row.trailingAnchor.constraint(equalTo: card.trailingAnchor, constant: -14),
            row.centerYAnchor.constraint(equalTo: card.centerYAnchor),
            row.heightAnchor.constraint(equalToConstant: 28),
        ])
        return card
    }

    /// Single Azad connector card (full pane width). Expands in-place to show
    /// voice-command help under the action buttons when toggled.
    private func azadConnectorCard(_ connector: ConnectorRow, index: Int) -> NSView {
        let appleIntelReady = connector.availabilityState == "available"
        // Command cheat-sheet stays available once AI is ready; only setup
        // actions (Check again / Open Settings) hide when the gate passes.
        let expandCommands = showAzadVoiceCommands

        let card = ThemedLayerView(fill: Design.panel, stroke: Design.border, radius: 8, borderWidth: 1)
        card.layer?.cornerRadius = 8
        card.setContentHuggingPriority(.required, for: .vertical)
        card.setContentCompressionResistancePriority(.required, for: .vertical)
        card.setContentHuggingPriority(.defaultLow, for: .horizontal)

        let top = NSStackView()
        top.orientation = .horizontal
        top.alignment = .centerY
        top.distribution = .fill
        top.spacing = 10
        top.translatesAutoresizingMaskIntoConstraints = false

        let checkbox = Design.checkbox("", checked: connector.enabled, target: self, action: #selector(toggleConnector(_:)))
        checkbox.state = connector.enabled ? .on : .off
        checkbox.isEnabled = connector.canEnable
        checkbox.tag = index
        pinIntrinsicVertical(checkbox)

        let logo = connectorLogo(for: connector)
        pinIntrinsicVertical(logo)
        let name = Design.label(connector.displayName, size: 13, weight: .medium)
        pinIntrinsicVertical(name)
        let spacer = NSView()
        spacer.translatesAutoresizingMaskIntoConstraints = false
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        spacer.setContentHuggingPriority(.required, for: .vertical)
        spacer.heightAnchor.constraint(equalToConstant: 1).isActive = true
        let pill = triggerPill(connector.trigger)
        pinIntrinsicVertical(pill)
        for v in [checkbox, logo, name, spacer, pill] {
            top.addArrangedSubview(v)
        }

        let status = connector.availabilityMessage.isEmpty
            ? "Say “hey azad …” to change text-replacement settings by voice."
            : connector.availabilityMessage
        let statusLabel = Design.label(status, size: 11, color: Design.secondaryText)
        statusLabel.lineBreakMode = .byTruncatingTail
        statusLabel.maximumNumberOfLines = 1
        statusLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        let actions = NSStackView()
        actions.orientation = .horizontal
        actions.spacing = 8
        actions.alignment = .centerY
        actions.distribution = .fill
        actions.translatesAutoresizingMaskIntoConstraints = false

        // Only show setup actions while Apple Intelligence is not ready.
        if !appleIntelReady {
            if connector.showOpenSettings {
                let openBtn = NSButton(
                    title: "Open Apple Intelligence Settings",
                    target: self,
                    action: #selector(openSystemSettings(_:))
                )
                openBtn.bezelStyle = .rounded
                openBtn.controlSize = .small
                actions.addArrangedSubview(openBtn)
            }
            let recheck = NSButton(title: "Check again", target: self, action: #selector(recheckAppleLm(_:)))
            recheck.bezelStyle = .rounded
            recheck.controlSize = .small
            actions.addArrangedSubview(recheck)
        }

        let actionSpacer = NSView()
        actionSpacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        actions.addArrangedSubview(actionSpacer)

        // Command list is always useful; expand in-card.
        let helpTitle = showAzadVoiceCommands ? "Hide commands" : "What can I say?"
        let help = NSButton(title: helpTitle, target: self, action: #selector(toggleAzadVoiceCommands(_:)))
        help.bezelStyle = .rounded
        help.controlSize = .small
        actions.addArrangedSubview(help)

        card.addSubview(top)
        card.addSubview(statusLabel)
        card.addSubview(actions)

        var constraints: [NSLayoutConstraint] = [
            top.leadingAnchor.constraint(equalTo: card.leadingAnchor, constant: 14),
            top.trailingAnchor.constraint(equalTo: card.trailingAnchor, constant: -14),
            top.topAnchor.constraint(equalTo: card.topAnchor, constant: 10),
            top.heightAnchor.constraint(equalToConstant: 28),

            statusLabel.leadingAnchor.constraint(equalTo: card.leadingAnchor, constant: 14),
            statusLabel.trailingAnchor.constraint(equalTo: card.trailingAnchor, constant: -14),
            statusLabel.topAnchor.constraint(equalTo: top.bottomAnchor, constant: 6),

            actions.leadingAnchor.constraint(equalTo: card.leadingAnchor, constant: 14),
            actions.trailingAnchor.constraint(equalTo: card.trailingAnchor, constant: -14),
            actions.topAnchor.constraint(equalTo: statusLabel.bottomAnchor, constant: 8),
            actions.heightAnchor.constraint(equalToConstant: 22),
        ]

        if expandCommands {
            let divider = ThemedLayerView(fill: Design.border)
            divider.translatesAutoresizingMaskIntoConstraints = false

            let title = Design.label("Voice commands (work without Apple Intelligence)", size: 12, weight: .semibold)
            let body = Design.wrappingLabel(
                """
                After “hey azad …”:
                · enable / disable number text replacement
                · enable / disable spoken emoji
                · enable / disable hesitations
                · enable / disable trailing space after paste
                · enable / disable repeated-word removal
                · enable / disable lowercase (except uppercase words)
                · add the word <word> to removed words
                · remove the word <word> from removed words

                Example: “hey azad, disable automatic number text replacement”
                """,
                size: 11,
                color: Design.secondaryText
            )
            body.maximumNumberOfLines = 0

            card.addSubview(divider)
            card.addSubview(title)
            card.addSubview(body)
            constraints.append(contentsOf: [
                divider.leadingAnchor.constraint(equalTo: card.leadingAnchor, constant: 14),
                divider.trailingAnchor.constraint(equalTo: card.trailingAnchor, constant: -14),
                divider.topAnchor.constraint(equalTo: actions.bottomAnchor, constant: 10),
                divider.heightAnchor.constraint(equalToConstant: 1),

                title.leadingAnchor.constraint(equalTo: card.leadingAnchor, constant: 14),
                title.trailingAnchor.constraint(equalTo: card.trailingAnchor, constant: -14),
                title.topAnchor.constraint(equalTo: divider.bottomAnchor, constant: 10),

                body.leadingAnchor.constraint(equalTo: card.leadingAnchor, constant: 14),
                body.trailingAnchor.constraint(equalTo: card.trailingAnchor, constant: -14),
                body.topAnchor.constraint(equalTo: title.bottomAnchor, constant: 6),
                body.bottomAnchor.constraint(equalTo: card.bottomAnchor, constant: -12),
            ])
        } else {
            constraints.append(
                actions.bottomAnchor.constraint(equalTo: card.bottomAnchor, constant: -10)
            )
        }

        NSLayoutConstraint.activate(constraints)
        return card
    }

    /// Prevent NSStackView from stretching chrome (logo, pill, checkbox) to row height.
    private func pinIntrinsicVertical(_ view: NSView) {
        view.setContentHuggingPriority(.required, for: .vertical)
        view.setContentCompressionResistancePriority(.required, for: .vertical)
    }

    /// Spotify connector card — gated on Spotify.app; in-card command list.
    private func spotifyConnectorCard(_ connector: ConnectorRow, index: Int) -> NSView {
        let installed = connector.availabilityState == "available"
        let expand = showSpotifyVoiceCommands

        let card = ThemedLayerView(fill: Design.panel, stroke: Design.border, radius: 8, borderWidth: 1)
        card.layer?.cornerRadius = 8
        card.setContentHuggingPriority(.required, for: .vertical)
        card.setContentHuggingPriority(.defaultLow, for: .horizontal)

        let top = NSStackView()
        top.orientation = .horizontal
        top.alignment = .centerY
        top.distribution = .fill
        top.spacing = 10
        top.translatesAutoresizingMaskIntoConstraints = false

        let checkbox = Design.checkbox("", checked: connector.enabled, target: self, action: #selector(toggleConnector(_:)))
        checkbox.state = connector.enabled ? .on : .off
        checkbox.isEnabled = connector.canEnable
        checkbox.tag = index
        pinIntrinsicVertical(checkbox)

        let logo = connectorLogo(for: connector)
        pinIntrinsicVertical(logo)
        let name = Design.label(connector.displayName, size: 13, weight: .medium)
        pinIntrinsicVertical(name)
        let spacer = NSView()
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        spacer.setContentHuggingPriority(.required, for: .vertical)
        spacer.heightAnchor.constraint(equalToConstant: 1).isActive = true
        let pill = triggerPill(connector.trigger)
        pinIntrinsicVertical(pill)
        for v in [checkbox, logo, name, spacer, pill] {
            top.addArrangedSubview(v)
        }

        let statusLabel = Design.label(connector.availabilityMessage, size: 11, color: Design.secondaryText)
        statusLabel.lineBreakMode = .byTruncatingTail
        statusLabel.maximumNumberOfLines = 1
        statusLabel.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        let actions = NSStackView()
        actions.orientation = .horizontal
        actions.spacing = 8
        actions.alignment = .centerY
        actions.translatesAutoresizingMaskIntoConstraints = false

        // Only show install / recheck while Spotify.app is missing.
        if !installed {
            let getApp = NSButton(title: "Get Spotify", target: self, action: #selector(openSpotifyDownload(_:)))
            getApp.bezelStyle = .rounded
            getApp.controlSize = .small
            actions.addArrangedSubview(getApp)
            let recheck = NSButton(title: "Check again", target: self, action: #selector(recheckSpotify(_:)))
            recheck.bezelStyle = .rounded
            recheck.controlSize = .small
            actions.addArrangedSubview(recheck)
        }

        let actionSpacer = NSView()
        actionSpacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        actions.addArrangedSubview(actionSpacer)

        if installed {
            let helpTitle = expand ? "Hide commands" : "What can I say?"
            let help = NSButton(title: helpTitle, target: self, action: #selector(toggleSpotifyVoiceCommands(_:)))
            help.bezelStyle = .rounded
            help.controlSize = .small
            actions.addArrangedSubview(help)
        }

        card.addSubview(top)
        card.addSubview(statusLabel)
        card.addSubview(actions)

        var constraints: [NSLayoutConstraint] = [
            top.leadingAnchor.constraint(equalTo: card.leadingAnchor, constant: 14),
            top.trailingAnchor.constraint(equalTo: card.trailingAnchor, constant: -14),
            top.topAnchor.constraint(equalTo: card.topAnchor, constant: 10),
            top.heightAnchor.constraint(equalToConstant: 28),
            statusLabel.leadingAnchor.constraint(equalTo: card.leadingAnchor, constant: 14),
            statusLabel.trailingAnchor.constraint(equalTo: card.trailingAnchor, constant: -14),
            statusLabel.topAnchor.constraint(equalTo: top.bottomAnchor, constant: 6),
            actions.leadingAnchor.constraint(equalTo: card.leadingAnchor, constant: 14),
            actions.trailingAnchor.constraint(equalTo: card.trailingAnchor, constant: -14),
            actions.topAnchor.constraint(equalTo: statusLabel.bottomAnchor, constant: 8),
            actions.heightAnchor.constraint(equalToConstant: 22),
        ]

        if expand && installed {
            let divider = ThemedLayerView(fill: Design.border)
            divider.translatesAutoresizingMaskIntoConstraints = false
            let title = Design.label("Voice commands", size: 12, weight: .semibold)
            let body = Design.wrappingLabel(
                """
                After “hey spotify …”:
                · pause / play / next / previous
                · play <song or artist>
                · what song is this / identify (Shazam — coming soon)
                · identify and play
                · volume up / volume down
                · current / what’s playing
                · like
                """,
                size: 11,
                color: Design.secondaryText
            )
            body.maximumNumberOfLines = 0
            card.addSubview(divider)
            card.addSubview(title)
            card.addSubview(body)
            constraints.append(contentsOf: [
                divider.leadingAnchor.constraint(equalTo: card.leadingAnchor, constant: 14),
                divider.trailingAnchor.constraint(equalTo: card.trailingAnchor, constant: -14),
                divider.topAnchor.constraint(equalTo: actions.bottomAnchor, constant: 10),
                divider.heightAnchor.constraint(equalToConstant: 1),
                title.leadingAnchor.constraint(equalTo: card.leadingAnchor, constant: 14),
                title.trailingAnchor.constraint(equalTo: card.trailingAnchor, constant: -14),
                title.topAnchor.constraint(equalTo: divider.bottomAnchor, constant: 10),
                body.leadingAnchor.constraint(equalTo: card.leadingAnchor, constant: 14),
                body.trailingAnchor.constraint(equalTo: card.trailingAnchor, constant: -14),
                body.topAnchor.constraint(equalTo: title.bottomAnchor, constant: 6),
                body.bottomAnchor.constraint(equalTo: card.bottomAnchor, constant: -12),
            ])
        } else {
            constraints.append(actions.bottomAnchor.constraint(equalTo: card.bottomAnchor, constant: -10))
        }
        NSLayoutConstraint.activate(constraints)
        return card
    }

    private func connectorLogo(for connector: ConnectorRow) -> NSView {
        let fill: NSColor
        let letter: String?
        switch connector.id {
        case "azad":
            fill = Design.accent
            letter = "A"
        case "spotify":
            fill = NSColor(calibratedRed: 0.114, green: 0.725, blue: 0.329, alpha: 1.0)
            letter = "♪"
        default:
            fill = Design.claude
            letter = nil
        }
        let container = ThemedLayerView(fill: fill, radius: 7)
        container.layer?.cornerRadius = 7
        container.widthAnchor.constraint(equalToConstant: 28).isActive = true
        container.heightAnchor.constraint(equalToConstant: 28).isActive = true
        container.setContentHuggingPriority(.required, for: .vertical)
        container.setContentHuggingPriority(.required, for: .horizontal)
        container.setContentCompressionResistancePriority(.required, for: .vertical)
        container.setContentCompressionResistancePriority(.required, for: .horizontal)

        if let letter {
            let label = Design.label(letter, size: 13, weight: .bold, color: .white)
            container.addSubview(label)
            NSLayoutConstraint.activate([
                label.centerXAnchor.constraint(equalTo: container.centerXAnchor),
                label.centerYAnchor.constraint(equalTo: container.centerYAnchor),
            ])
        } else {
            let logo = Design.claudeLogoView(size: 18)
            container.addSubview(logo)
            NSLayoutConstraint.activate([
                logo.centerXAnchor.constraint(equalTo: container.centerXAnchor),
                logo.centerYAnchor.constraint(equalTo: container.centerYAnchor),
            ])
        }
        return container
    }

    private func triggerPill(_ text: String) -> NSView {
        let container = ThemedLayerView(fill: Design.chip, radius: 6)
        container.layer?.cornerRadius = 6
        container.heightAnchor.constraint(equalToConstant: 24).isActive = true
        container.setContentHuggingPriority(.required, for: .vertical)
        container.setContentHuggingPriority(.required, for: .horizontal)
        container.setContentCompressionResistancePriority(.required, for: .vertical)
        container.setContentCompressionResistancePriority(.required, for: .horizontal)

        let label = Design.label(text, size: 12, color: Design.secondaryText)
        label.alignment = .center
        label.setContentHuggingPriority(.required, for: .horizontal)
        container.addSubview(label)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 10),
            label.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -10),
            label.centerYAnchor.constraint(equalTo: container.centerYAnchor),
            container.widthAnchor.constraint(greaterThanOrEqualToConstant: 100),
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

    @objc private func selectStartupListenMode(_ sender: NSPopUpButton) {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "selectStartupListenMode", index: sender.indexOfSelectedItem))
    }

    @objc private func setActivationLevel(_ sender: NSSlider) {
        let value = sender.integerValue
        activationLevelValueLabel?.stringValue = activationLevelLabel(for: value)
        if let model {
            self.model = SettingsViewModel(
                selectedTab: model.selectedTab,
                accessibilityStatus: model.accessibilityStatus,
                microphoneStatus: model.microphoneStatus,
                runOnStartupEnabled: model.runOnStartupEnabled,
                startupListenModeIndex: model.startupListenModeIndex,
                activationLevel: value,
                historyEnabled: model.historyEnabled,
                pasteMethodIndex: model.pasteMethodIndex,
                autoSubmitIndex: model.autoSubmitIndex,
                overlayPositionIndex: model.overlayPositionIndex,
                appendTrailingSpaceOnPaste: model.appendTrailingSpaceOnPaste,
                deduplicateWordsOnPaste: model.deduplicateWordsOnPaste,
                convertNumberWordsOnPaste: model.convertNumberWordsOnPaste,
                convertSpokenEmojiOnPaste: model.convertSpokenEmojiOnPaste,
                lowercaseExceptUppercaseWordsOnPaste: model.lowercaseExceptUppercaseWordsOnPaste,
                removeHesitationsOnPaste: model.removeHesitationsOnPaste,
                listenModifiers: model.listenModifiers,
                debugStatsEnabled: model.debugStatsEnabled,
                metricsText: model.metricsText,
                model: model.model,
                removedWords: model.removedWords,
                connectors: model.connectors,
                buildInfo: model.buildInfo
            )
        }
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "setActivationLevel", index: value))
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

    @objc private func toggleHistory(_ sender: NSButton) {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "toggleHistory", boolValue: sender.state == .on))
    }

    @objc private func toggleTrailingSpace(_ sender: NSButton) {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "toggleAppendTrailingSpace", boolValue: sender.state == .on))
    }

    @objc private func toggleDeduplicateWords(_ sender: NSButton) {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "toggleDeduplicateWords", boolValue: sender.state == .on))
    }

    @objc private func toggleConvertNumberWords(_ sender: NSButton) {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "toggleConvertNumberWords", boolValue: sender.state == .on))
    }

    @objc private func toggleConvertSpokenEmoji(_ sender: NSButton) {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "toggleConvertSpokenEmoji", boolValue: sender.state == .on))
    }

    @objc private func toggleLowercaseExceptUppercaseWords(_ sender: NSButton) {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "toggleLowercaseExceptUppercaseWords", boolValue: sender.state == .on))
    }

    @objc private func toggleRemoveHesitations(_ sender: NSButton) {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "toggleRemoveHesitations", boolValue: sender.state == .on))
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

    @objc private func controlDownload(_ sender: NSButton) {
        let action = sender.tag == 1 ? "resumeDownload" : "pauseDownload"
        AzadUI.shared.emit(UIEvent(surface: "settings", action: action))
    }

    @objc private func openPermission(_ sender: NSButton) {
        guard let permission = sender.identifier?.rawValue else { return }
        if sender.tag == permissionRequestButtonTag {
            performNativePermissionRequest(permission) {
                AzadUI.shared.emit(UIEvent(surface: "settings", action: "requestPermission", permission: permission))
            }
            return
        }
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "openPermission", permission: permission))
    }

    @objc private func toggleConnector(_ sender: NSButton) {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "toggleConnector", boolValue: sender.state == .on, index: sender.tag))
    }

    @objc private func openSystemSettings(_ sender: NSButton) {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "openSystemSettings"))
    }

    @objc private func recheckAppleLm(_ sender: NSButton) {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "recheckAppleLm"))
    }

    @objc private func toggleAzadVoiceCommands(_ sender: NSButton) {
        showAzadVoiceCommands.toggle()
        render()
    }

    @objc private func toggleSpotifyVoiceCommands(_ sender: NSButton) {
        showSpotifyVoiceCommands.toggle()
        render()
    }

    @objc private func openSpotifyDownload(_ sender: NSButton) {
        if let url = URL(string: "https://www.spotify.com/download/") {
            NSWorkspace.shared.open(url)
        }
    }

    @objc private func recheckSpotify(_ sender: NSButton) {
        AzadUI.shared.emit(UIEvent(surface: "settings", action: "refresh"))
    }

    @objc private func addWord(_ sender: AnyObject) {
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
    private let selected: Bool
    private let iconView = PassthroughImageView()
    private let titleLabel = PassthroughTextField(labelWithString: "")

    init(tab: SettingsTab, icon: String, title: String, selected: Bool, target: AnyObject?, action: Selector?) {
        self.tab = tab
        self.selected = selected
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

        iconView.image = NSImage(systemSymbolName: icon, accessibilityDescription: nil)
        iconView.translatesAutoresizingMaskIntoConstraints = false
        addSubview(iconView)

        titleLabel.stringValue = title
        titleLabel.font = .systemFont(ofSize: 14, weight: selected ? .semibold : .regular)
        titleLabel.translatesAutoresizingMaskIntoConstraints = false
        addSubview(titleLabel)
        updateAppearance()

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

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        updateAppearance()
    }

    private func updateAppearance() {
        layer?.backgroundColor = selected ? Design.cgColor(Design.blue, for: self) : NSColor.clear.cgColor
        iconView.contentTintColor = selected ? .white : Design.secondaryText
        titleLabel.textColor = selected ? .white : Design.secondaryText
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

final class WrappingChipsView: NSView {
    private let horizontalSpacing: CGFloat = 6
    private let verticalSpacing: CGFloat = 8
    private var heightConstraint: NSLayoutConstraint?
    override var isFlipped: Bool { true }

    init(words: [String], target: AnyObject?, action: Selector?) {
        super.init(frame: .zero)
        translatesAutoresizingMaskIntoConstraints = false
        for word in words {
            let chip = WordChip(word: word, target: target, action: action)
            chip.translatesAutoresizingMaskIntoConstraints = true
            addSubview(chip)
        }
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func viewDidMoveToSuperview() {
        super.viewDidMoveToSuperview()
        if heightConstraint == nil {
            let constraint = heightAnchor.constraint(equalToConstant: measuredHeight(for: bounds.width))
            constraint.isActive = true
            heightConstraint = constraint
        }
    }

    override func layout() {
        super.layout()
        let size = layoutChips(width: bounds.width, apply: true)
        if abs((heightConstraint?.constant ?? 0) - size.height) > 0.5 {
            heightConstraint?.constant = size.height
            invalidateIntrinsicContentSize()
        }
    }

    override var intrinsicContentSize: NSSize {
        NSSize(width: NSView.noIntrinsicMetric, height: measuredHeight(for: bounds.width))
    }

    private func measuredHeight(for width: CGFloat) -> CGFloat {
        layoutChips(width: width > 0 ? width : 320, apply: false).height
    }

    private func layoutChips(width: CGFloat, apply: Bool) -> NSSize {
        let maxWidth = max(width, 54)
        var x: CGFloat = 0
        var y: CGFloat = 0
        var rowHeight: CGFloat = 0
        var usedWidth: CGFloat = 0

        for chip in subviews {
            let chipSize = chip.fittingSize
            let chipWidth = min(max(chipSize.width, 54), maxWidth)
            let chipHeight = max(chipSize.height, 28)
            if x > 0, x + chipWidth > maxWidth {
                x = 0
                y += rowHeight + verticalSpacing
                rowHeight = 0
            }
            if apply {
                chip.frame = NSRect(x: x, y: y, width: chipWidth, height: chipHeight)
            }
            x += chipWidth + horizontalSpacing
            usedWidth = max(usedWidth, min(x - horizontalSpacing, maxWidth))
            rowHeight = max(rowHeight, chipHeight)
        }

        return NSSize(width: usedWidth, height: subviews.isEmpty ? 0 : y + rowHeight)
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
        self.translatesAutoresizingMaskIntoConstraints = false
        self.wantsLayer = true
        self.layer?.cornerRadius = 13
        self.heightAnchor.constraint(equalToConstant: 28).isActive = true
        self.widthAnchor.constraint(greaterThanOrEqualToConstant: 54).isActive = true
        updateAppearance()
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        updateAppearance()
    }

    private func updateAppearance() {
        contentTintColor = Design.text
        layer?.backgroundColor = Design.cgColor(Design.chip, for: self)
    }
}
