import AppKit

let modifierShift: UInt8 = 1
let modifierControl: UInt8 = 2
let modifierOption: UInt8 = 4
let modifierCommand: UInt8 = 8

final class KeycapButton: NSButton {
    let bit: UInt8

    override var state: NSControl.StateValue {
        didSet { updateAppearance() }
    }

    override var isHighlighted: Bool {
        didSet { updateAppearance() }
    }

    init(title: String, bit: UInt8, mask: UInt8, target: AnyObject?, action: Selector?) {
        self.bit = bit
        super.init(frame: .zero)
        self.title = title
        self.target = target
        self.action = action
        setButtonType(.toggle)
        isBordered = false
        wantsLayer = true
        layer?.cornerRadius = 15
        layer?.borderWidth = 1
        controlSize = .large
        font = .systemFont(ofSize: 15, weight: .medium)
        state = (mask & bit) != 0 ? .on : .off
        translatesAutoresizingMaskIntoConstraints = false
        widthAnchor.constraint(equalToConstant: 34).isActive = true
        heightAnchor.constraint(equalToConstant: 30).isActive = true
        updateAppearance()
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    private func updateAppearance() {
        let selected = state == .on
        let background = selected
            ? (isHighlighted ? Design.blue.withAlphaComponent(0.82) : Design.blue)
            : Design.control
        let foreground = selected ? NSColor.white : Design.text
        layer?.backgroundColor = background.cgColor
        layer?.borderColor = selected ? Design.blue.cgColor : Design.border.cgColor
        attributedTitle = NSAttributedString(
            string: title,
            attributes: [
                .font: NSFont.systemFont(ofSize: 15, weight: .medium),
                .foregroundColor: foreground,
            ]
        )
    }
}

final class ShortcutView: NSStackView {
    private var buttons: [KeycapButton] = []

    init(mask: UInt8, target: AnyObject?, action: Selector?) {
        super.init(frame: .zero)
        orientation = .horizontal
        alignment = .centerY
        spacing = 6
        translatesAutoresizingMaskIntoConstraints = false

        let specs: [(String, UInt8)] = [
            ("⇧", modifierShift),
            ("⌃", modifierControl),
            ("⌥", modifierOption),
            ("⌘", modifierCommand),
        ]
        for spec in specs {
            let button = KeycapButton(title: spec.0, bit: spec.1, mask: mask, target: target, action: action)
            buttons.append(button)
            addArrangedSubview(button)
        }
        addArrangedSubview(Design.label("+", size: 13, color: Design.mutedText))
        let space = Design.label("Space", size: 13, weight: .medium, color: Design.text)
        space.alignment = .center
        space.wantsLayer = true
        space.layer?.backgroundColor = Design.control.cgColor
        space.layer?.borderColor = Design.border.cgColor
        space.layer?.borderWidth = 1
        space.layer?.cornerRadius = 6
        space.widthAnchor.constraint(equalToConstant: 78).isActive = true
        space.heightAnchor.constraint(equalToConstant: 30).isActive = true
        addArrangedSubview(space)
    }

    required init(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    func sync(mask: UInt8) {
        for button in buttons {
            button.state = (mask & button.bit) != 0 ? .on : .off
        }
    }
}

final class FormRow: NSStackView {
    init(label: String, labelWidth: CGFloat = 150, control: NSView) {
        super.init(frame: .zero)
        orientation = .horizontal
        alignment = .centerY
        spacing = 14
        translatesAutoresizingMaskIntoConstraints = false

        let labelView = Design.label(label, size: 13, color: Design.secondaryText)
        labelView.alignment = .right
        labelView.widthAnchor.constraint(equalToConstant: labelWidth).isActive = true
        addArrangedSubview(labelView)
        addArrangedSubview(control)
    }

    required init(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }
}

final class StatusTextView: NSStackView {
    init(text: String, status: PermissionStatus, iconName: String? = nil) {
        super.init(frame: .zero)
        orientation = .horizontal
        alignment = .centerY
        spacing = 7
        translatesAutoresizingMaskIntoConstraints = false

        if let iconName {
            addArrangedSubview(Design.symbol(iconName, pointSize: 13, color: Design.secondaryText))
        }

        addArrangedSubview(Design.label(text, size: 13, color: Design.text))
        let dot = status == .granted ? "checkmark.circle.fill" : "circle.fill"
        let color = status == .granted ? Design.green : Design.orange
        addArrangedSubview(Design.symbol(dot, pointSize: 12, color: color))
        addArrangedSubview(Design.label(status == .granted ? "Granted" : "Not granted", size: 13, weight: .medium, color: color))
    }

    required init(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }
}

final class PermissionCard: NSView {
    init(
        accessibility: PermissionStatus,
        microphone: PermissionStatus,
        target: AnyObject?,
        action: Selector?,
        compactGranted: Bool = false,
        showMissingHint: Bool = true
    ) {
        super.init(frame: .zero)
        translatesAutoresizingMaskIntoConstraints = false
        wantsLayer = true
        layer?.backgroundColor = Design.panel.cgColor
        layer?.cornerRadius = 8
        layer?.borderColor = Design.border.cgColor
        layer?.borderWidth = 1

        let stack = NSStackView()
        stack.orientation = .vertical
        stack.spacing = 0
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        stack.pinToSuperview(NSEdgeInsets(top: 10, left: 14, bottom: 10, right: 14))

        addPermissionRow(
            to: stack,
            label: "Accessibility",
            icon: "person.fill",
            status: accessibility,
            permission: "accessibility",
            target: target,
            action: action,
            showButton: !compactGranted && accessibility != .granted
        )
        stack.addArrangedSubview(Design.separatorView())
        addPermissionRow(
            to: stack,
            label: "Microphone",
            icon: "mic.fill",
            status: microphone,
            permission: "microphone",
            target: target,
            action: action,
            showButton: !compactGranted && microphone != .granted
        )

        if showMissingHint && !compactGranted && (accessibility != .granted || microphone != .granted) {
            let hint = Design.label("Microphone and Accessibility are required to use Azad.", size: 12, color: Design.mutedText)
            hint.translatesAutoresizingMaskIntoConstraints = false
            stack.addArrangedSubview(hint)
        }
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    private func addPermissionRow(
        to stack: NSStackView,
        label: String,
        icon: String,
        status: PermissionStatus,
        permission: String,
        target: AnyObject?,
        action: Selector?,
        showButton: Bool
    ) {
        let row = NSStackView()
        row.orientation = .horizontal
        row.alignment = .centerY
        row.spacing = 8
        row.translatesAutoresizingMaskIntoConstraints = false
        row.heightAnchor.constraint(equalToConstant: 32).isActive = true
        row.addArrangedSubview(Design.symbol(icon, pointSize: 12, color: Design.secondaryText))
        row.addArrangedSubview(Design.label(label, size: 13, color: Design.text))
        row.addArrangedSubview(NSView())
        row.arrangedSubviews.last?.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let statusView = StatusTextView(text: "", status: status)
        if let first = statusView.arrangedSubviews.first {
            statusView.removeArrangedSubview(first)
            first.removeFromSuperview()
        }
        row.addArrangedSubview(statusView)
        if showButton {
            let button = Design.pushButton("Open Settings", target: target, action: action)
            button.identifier = NSUserInterfaceItemIdentifier(permission)
            button.widthAnchor.constraint(equalToConstant: 120).isActive = true
            row.addArrangedSubview(button)
        }
        stack.addArrangedSubview(row)
    }
}

final class ModelRowView: NSView {
    init(model: ModelPack, compact: Bool, target: AnyObject?, downloadAction: Selector?, cancelAction: Selector?) {
        super.init(frame: .zero)
        translatesAutoresizingMaskIntoConstraints = false

        let stack = NSStackView()
        stack.orientation = compact ? .horizontal : .vertical
        stack.alignment = compact ? .centerY : .leading
        stack.spacing = compact ? 12 : 10
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        stack.pinToSuperview()

        let textStack = NSStackView()
        textStack.orientation = .vertical
        textStack.alignment = .leading
        textStack.spacing = 2
        textStack.translatesAutoresizingMaskIntoConstraints = false

        if compact {
            let title = Design.label("\(model.welcomeName) ↗", size: 13, weight: .medium, color: Design.blue)
            textStack.addArrangedSubview(title)
        }

        switch model.status {
        case .notDownloaded:
            textStack.addArrangedSubview(Design.label("\(model.sizeLabel) \u{00B7} Not downloaded", size: 12, color: Design.secondaryText))
        case .ready:
            textStack.addArrangedSubview(Design.label("\(model.sizeLabel) \u{00B7} Installed", size: 12, color: Design.secondaryText))
        case .downloading:
            textStack.addArrangedSubview(Design.label("Downloading... \(model.bytesDoneLabel) of \(model.bytesTotalLabel) (\(model.progressPct)%)", size: 12, color: Design.secondaryText))
        case .failed:
            textStack.addArrangedSubview(Design.label(model.errorMessage.isEmpty ? "Download failed" : model.errorMessage, size: 12, color: Design.red))
        }
        stack.addArrangedSubview(textStack)

        let actionStack: NSStackView
        if compact {
            stack.addArrangedSubview(NSView())
            actionStack = stack
        } else {
            actionStack = NSStackView()
            actionStack.orientation = .horizontal
            actionStack.alignment = .centerY
            actionStack.spacing = 12
            actionStack.translatesAutoresizingMaskIntoConstraints = false
            stack.addArrangedSubview(actionStack)
            actionStack.widthAnchor.constraint(equalTo: stack.widthAnchor).isActive = true
        }

        switch model.status {
        case .notDownloaded:
            let button = Design.pushButton("Download", target: target, action: downloadAction)
            button.widthAnchor.constraint(equalToConstant: 96).isActive = true
            actionStack.addArrangedSubview(button)
        case .downloading:
            let progress = NSProgressIndicator()
            progress.isIndeterminate = false
            progress.minValue = 0
            progress.maxValue = 100
            progress.doubleValue = Double(model.progressPct)
            progress.controlSize = .small
            progress.translatesAutoresizingMaskIntoConstraints = false
            progress.widthAnchor.constraint(equalToConstant: compact ? 160 : 260).isActive = true
            actionStack.addArrangedSubview(progress)
            let button = Design.pushButton("Cancel", target: target, action: cancelAction)
            button.widthAnchor.constraint(equalToConstant: 82).isActive = true
            actionStack.addArrangedSubview(button)
        case .ready:
            let status = StatusTextView(text: "", status: .granted)
            if let first = status.arrangedSubviews.first {
                status.removeArrangedSubview(first)
                first.removeFromSuperview()
            }
            if let label = status.arrangedSubviews.last as? NSTextField {
                label.stringValue = "Ready"
            }
            actionStack.addArrangedSubview(status)
        case .failed:
            let button = Design.pushButton("Retry", target: target, action: downloadAction)
            button.widthAnchor.constraint(equalToConstant: 82).isActive = true
            actionStack.addArrangedSubview(button)
        }
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }
}
