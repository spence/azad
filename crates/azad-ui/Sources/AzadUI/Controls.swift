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
        layer?.backgroundColor = Design.cgColor(background, for: self)
        layer?.borderColor = Design.cgColor(selected ? Design.blue : Design.border, for: self)
        attributedTitle = NSAttributedString(
            string: title,
            attributes: [
                .font: NSFont.systemFont(ofSize: 15, weight: .medium),
                .foregroundColor: foreground,
            ]
        )
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        updateAppearance()
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
        let space = ThemedLayerView(fill: Design.control, stroke: Design.border, radius: 6, borderWidth: 1)
        space.layer?.borderWidth = 1
        space.layer?.cornerRadius = 6
        space.widthAnchor.constraint(equalToConstant: 78).isActive = true
        space.heightAnchor.constraint(equalToConstant: 30).isActive = true

        let spaceLabel = Design.label("Space", size: 13, weight: .medium, color: Design.text)
        spaceLabel.alignment = .center
        space.addSubview(spaceLabel)
        NSLayoutConstraint.activate([
            spaceLabel.centerXAnchor.constraint(equalTo: space.centerXAnchor),
            spaceLabel.centerYAnchor.constraint(equalTo: space.centerYAnchor),
        ])
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
        spacing = 5
        translatesAutoresizingMaskIntoConstraints = false

        if let iconName {
            addArrangedSubview(Design.symbol(iconName, pointSize: 12, color: Design.secondaryText))
        }

        addArrangedSubview(Design.label(text, size: 12, color: Design.text))
        let color = status.statusColor
        addArrangedSubview(Design.symbol(status.statusIconName, pointSize: 10, color: color))
        addArrangedSubview(Design.label(status.statusText, size: 12, weight: .medium, color: color))
    }

    required init(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }
}

final class PermissionCard: NSView {
    private let framed: Bool

    init(
        accessibility: PermissionStatus,
        microphone: PermissionStatus,
        target: AnyObject?,
        action: Selector?,
        compactGranted: Bool = false,
        showMissingHint: Bool = true,
        framed: Bool = true
    ) {
        self.framed = framed
        super.init(frame: .zero)
        translatesAutoresizingMaskIntoConstraints = false
        if framed {
            wantsLayer = true
            layer?.cornerRadius = 8
            layer?.borderWidth = 1
            applyTheme()
        }

        let stack = NSStackView()
        stack.orientation = .vertical
        stack.spacing = 0
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        let insets = framed
            ? NSEdgeInsets(top: 10, left: 14, bottom: 10, right: 14)
            : NSEdgeInsets(top: 0, left: 0, bottom: 0, right: 0)
        stack.pinToSuperview(insets)

        addPermissionRow(
            to: stack,
            label: "Accessibility",
            icon: "person.fill",
            status: accessibility,
            permission: "accessibility",
            target: target,
            action: action,
            showButton: !compactGranted && !accessibility.isGranted
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
            showButton: !compactGranted && !microphone.isGranted
        )

        if showMissingHint && !compactGranted && (!accessibility.isGranted || !microphone.isGranted) {
            let hint = Design.label("Microphone and Accessibility are required to use Azad.", size: 12, color: Design.mutedText)
            hint.translatesAutoresizingMaskIntoConstraints = false
            stack.addArrangedSubview(hint)
        }
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        applyTheme()
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        applyTheme()
    }

    private func applyTheme() {
        guard framed else { return }
        Design.applyFill(Design.panel, to: self)
        Design.applyStroke(Design.border, to: self)
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
        row.heightAnchor.constraint(equalToConstant: 28).isActive = true
        row.addArrangedSubview(Design.symbol(icon, pointSize: 11, color: Design.secondaryText))
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
            let button = Design.pushButton(status.actionTitle, target: target, action: action)
            button.controlSize = .regular
            button.font = .systemFont(ofSize: 12, weight: .medium)
            button.identifier = NSUserInterfaceItemIdentifier(permission)
            button.tag = status.requestsPermission ? 1 : 0
            button.widthAnchor.constraint(equalToConstant: 104).isActive = true
            button.heightAnchor.constraint(equalToConstant: 24).isActive = true
            row.addArrangedSubview(button)
        }
        stack.addArrangedSubview(row)
    }
}

final class ModelRowView: NSView {
    private let contentStack = NSStackView()
    private let compactWidth: CGFloat = 358
    private let compactTextWidth: CGFloat = 248

    override var intrinsicContentSize: NSSize {
        NSSize(width: NSView.noIntrinsicMetric, height: contentStack.fittingSize.height)
    }

    init(model: ModelPack, compact: Bool, target: AnyObject?, downloadAction: Selector?, downloadControlAction: Selector?) {
        super.init(frame: .zero)
        translatesAutoresizingMaskIntoConstraints = false
        setContentHuggingPriority(.required, for: .vertical)
        setContentCompressionResistancePriority(.required, for: .vertical)
        if compact {
            widthAnchor.constraint(equalToConstant: compactWidth).isActive = true
        }

        let stack = contentStack
        stack.orientation = compact ? .horizontal : .vertical
        stack.alignment = compact ? .centerY : .leading
        stack.spacing = compact ? 12 : 10
        stack.translatesAutoresizingMaskIntoConstraints = false
        stack.setContentHuggingPriority(.required, for: .vertical)
        stack.setContentCompressionResistancePriority(.required, for: .vertical)
        addSubview(stack)
        stack.pinToSuperview()

        let textStack = NSStackView()
        textStack.orientation = .vertical
        textStack.alignment = .leading
        textStack.spacing = 2
        textStack.translatesAutoresizingMaskIntoConstraints = false
        if compact {
            textStack.widthAnchor.constraint(equalToConstant: compactTextWidth).isActive = true
            textStack.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        }

        if compact {
            let title = Design.linkLabel("\(model.welcomeName) ↗", url: model.pageUrl, size: 13, weight: .medium)
            textStack.addArrangedSubview(title)
        }

        switch model.status {
        case .notDownloaded:
            textStack.addArrangedSubview(Design.label("\(model.sizeLabel) \u{00B7} Not downloaded", size: 12, color: Design.secondaryText))
        case .ready:
            textStack.addArrangedSubview(Design.label("\(model.sizeLabel) \u{00B7} Installed", size: 12, color: Design.secondaryText))
        case .resumable:
            textStack.addArrangedSubview(Design.label("\(model.bytesDoneLabel) of \(model.bytesTotalLabel) \u{00B7} Ready to resume", size: 12, color: Design.secondaryText))
            if compact {
                let progress = ModelRowView.progressIndicator(model.progressPct, width: compactTextWidth)
                textStack.addArrangedSubview(progress)
            }
        case .downloading:
            let label = model.downloadPaused ? "Paused" : "Downloading..."
            textStack.addArrangedSubview(Design.label("\(label) \(model.bytesDoneLabel) of \(model.bytesTotalLabel) (\(model.progressPct)%)", size: 12, color: Design.secondaryText))
            if compact {
                let progress = ModelRowView.progressIndicator(model.progressPct, width: compactTextWidth)
                textStack.addArrangedSubview(progress)
            }
        case .failed:
            textStack.addArrangedSubview(Design.label(model.errorMessage.isEmpty ? "Download failed" : model.errorMessage, size: 12, color: Design.red))
        }
        stack.addArrangedSubview(textStack)

        let actionStack: NSStackView
        if compact {
            let spacer = NSView()
            spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
            stack.addArrangedSubview(spacer)

            actionStack = NSStackView()
            actionStack.orientation = .horizontal
            actionStack.alignment = .centerY
            actionStack.spacing = 8
            actionStack.translatesAutoresizingMaskIntoConstraints = false
            actionStack.setContentHuggingPriority(.required, for: .horizontal)
            stack.addArrangedSubview(actionStack)
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
        case .resumable:
            if !compact {
                actionStack.addArrangedSubview(ModelRowView.progressIndicator(model.progressPct, width: 260))
            }
            let button = Design.pushButton("Resume", target: target, action: downloadAction)
            button.widthAnchor.constraint(equalToConstant: 82).isActive = true
            actionStack.addArrangedSubview(button)
        case .downloading:
            if !compact {
                actionStack.addArrangedSubview(ModelRowView.progressIndicator(model.progressPct, width: 260))
            }
            let button = Design.pushButton(model.downloadPaused ? "Resume" : "Pause", target: target, action: downloadControlAction)
            button.tag = model.downloadPaused ? 1 : 0
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
        invalidateIntrinsicContentSize()
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    private static func progressIndicator(_ percent: UInt8, width: CGFloat) -> NSProgressIndicator {
        let progress = NSProgressIndicator()
        progress.isIndeterminate = false
        progress.minValue = 0
        progress.maxValue = 100
        progress.doubleValue = Double(percent)
        progress.controlSize = .small
        progress.translatesAutoresizingMaskIntoConstraints = false
        progress.widthAnchor.constraint(equalToConstant: width).isActive = true
        return progress
    }
}
