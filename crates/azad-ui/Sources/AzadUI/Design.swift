import AppKit

private final class AzadUIBundleToken {}

final class PrimaryActionButton: NSButton {
    override var isEnabled: Bool {
        didSet { updateAppearance() }
    }

    override var isHighlighted: Bool {
        didSet { updateAppearance() }
    }

    init(title: String, target: AnyObject?, action: Selector?) {
        super.init(frame: .zero)
        self.title = title
        self.target = target
        self.action = action
        isBordered = false
        wantsLayer = true
        layer?.cornerRadius = 7
        controlSize = .large
        font = .systemFont(ofSize: 13, weight: .semibold)
        translatesAutoresizingMaskIntoConstraints = false
        heightAnchor.constraint(equalToConstant: 34).isActive = true
        updateAppearance()
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    private func updateAppearance() {
        let background: NSColor
        let foreground: NSColor
        if isEnabled {
            background = isHighlighted ? Design.blue.withAlphaComponent(0.82) : Design.blue
            foreground = .white
        } else {
            background = Design.control
            foreground = Design.mutedText
        }
        layer?.backgroundColor = background.cgColor
        attributedTitle = NSAttributedString(
            string: title,
            attributes: [
                .font: NSFont.systemFont(ofSize: 13, weight: .semibold),
                .foregroundColor: foreground,
            ]
        )
    }
}

enum Design {
    static let window = NSColor(calibratedRed: 0.132, green: 0.132, blue: 0.145, alpha: 1.0)
    static let panel = NSColor(calibratedRed: 0.112, green: 0.112, blue: 0.124, alpha: 1.0)
    static let control = NSColor(calibratedRed: 0.205, green: 0.205, blue: 0.218, alpha: 1.0)
    static let border = NSColor(calibratedWhite: 1.0, alpha: 0.105)
    static let separator = NSColor(calibratedWhite: 1.0, alpha: 0.10)
    static let text = NSColor(calibratedWhite: 0.88, alpha: 1.0)
    static let secondaryText = NSColor(calibratedWhite: 0.62, alpha: 1.0)
    static let mutedText = NSColor(calibratedWhite: 0.42, alpha: 1.0)
    static let blue = NSColor.systemBlue
    static let green = NSColor.systemGreen
    static let orange = NSColor.systemOrange
    static let red = NSColor.systemRed

    static func label(_ text: String, size: CGFloat = 13, weight: NSFont.Weight = .regular, color: NSColor = Design.text) -> NSTextField {
        let label = NSTextField(labelWithString: text)
        label.font = .systemFont(ofSize: size, weight: weight)
        label.textColor = color
        label.lineBreakMode = .byTruncatingTail
        label.translatesAutoresizingMaskIntoConstraints = false
        return label
    }

    static func wrappingLabel(_ text: String, size: CGFloat = 13, weight: NSFont.Weight = .regular, color: NSColor = Design.secondaryText) -> NSTextField {
        let label = Self.label(text, size: size, weight: weight, color: color)
        label.maximumNumberOfLines = 0
        label.lineBreakMode = .byWordWrapping
        return label
    }

    static func popup(_ items: [String], selected: Int, target: AnyObject?, action: Selector?) -> NSPopUpButton {
        let popup = NSPopUpButton(frame: .zero, pullsDown: false)
        popup.addItems(withTitles: items)
        if selected >= 0 && selected < items.count {
            popup.selectItem(at: selected)
        }
        popup.target = target
        popup.action = action
        popup.controlSize = .large
        popup.translatesAutoresizingMaskIntoConstraints = false
        popup.heightAnchor.constraint(equalToConstant: 30).isActive = true
        return popup
    }

    static func checkbox(_ title: String, checked: Bool, target: AnyObject?, action: Selector?) -> NSButton {
        let button = NSButton(checkboxWithTitle: title, target: target, action: action)
        button.state = checked ? .on : .off
        button.font = .systemFont(ofSize: 13)
        button.contentTintColor = Design.blue
        button.attributedTitle = NSAttributedString(
            string: title,
            attributes: [
                .font: NSFont.systemFont(ofSize: 13),
                .foregroundColor: Design.text,
            ]
        )
        button.translatesAutoresizingMaskIntoConstraints = false
        return button
    }

    static func pushButton(_ title: String, target: AnyObject?, action: Selector?) -> NSButton {
        let button = NSButton(title: title, target: target, action: action)
        button.bezelStyle = .rounded
        button.controlSize = .large
        button.font = .systemFont(ofSize: 13, weight: .medium)
        button.translatesAutoresizingMaskIntoConstraints = false
        return button
    }

    static func primaryButton(_ title: String, target: AnyObject?, action: Selector?) -> NSButton {
        PrimaryActionButton(title: title, target: target, action: action)
    }

    static func symbol(_ name: String, pointSize: CGFloat = 15, color: NSColor = Design.secondaryText) -> NSImageView {
        let config = NSImage.SymbolConfiguration(pointSize: pointSize, weight: .regular)
        let image = NSImage(systemSymbolName: name, accessibilityDescription: nil)?.withSymbolConfiguration(config)
        let view = NSImageView(image: image ?? NSImage())
        view.contentTintColor = color
        view.translatesAutoresizingMaskIntoConstraints = false
        view.widthAnchor.constraint(equalToConstant: pointSize + 4).isActive = true
        view.heightAnchor.constraint(equalToConstant: pointSize + 4).isActive = true
        return view
    }

    static func appIconView(size: CGFloat = 32) -> NSView {
        let container = NSView()
        container.wantsLayer = true
        container.layer?.backgroundColor = Design.control.cgColor
        container.layer?.cornerRadius = 8
        container.translatesAutoresizingMaskIntoConstraints = false
        container.widthAnchor.constraint(equalToConstant: size).isActive = true
        container.heightAnchor.constraint(equalToConstant: size).isActive = true

        let imageView = NSImageView(image: appIconImage())
        imageView.imageScaling = .scaleProportionallyUpOrDown
        imageView.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(imageView)
        NSLayoutConstraint.activate([
            imageView.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 6),
            imageView.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -6),
            imageView.topAnchor.constraint(equalTo: container.topAnchor, constant: 5),
            imageView.bottomAnchor.constraint(equalTo: container.bottomAnchor, constant: -5),
        ])

        return container
    }

    static func roundedPanel(radius: CGFloat = 8) -> NSView {
        let view = NSView()
        view.wantsLayer = true
        view.layer?.backgroundColor = Design.panel.cgColor
        view.layer?.cornerRadius = radius
        view.layer?.borderColor = Design.border.cgColor
        view.layer?.borderWidth = 1
        view.translatesAutoresizingMaskIntoConstraints = false
        return view
    }

    static func separatorView() -> NSView {
        let view = NSView()
        view.wantsLayer = true
        view.layer?.backgroundColor = Design.separator.cgColor
        view.translatesAutoresizingMaskIntoConstraints = false
        view.heightAnchor.constraint(equalToConstant: 1).isActive = true
        return view
    }

    private static func appIconImage() -> NSImage {
        if let url = Bundle.main.url(forResource: "azad-white", withExtension: "png"),
           let image = NSImage(contentsOf: url) {
            return image
        }
        if let url = Bundle(for: AzadUIBundleToken.self).url(forResource: "azad-white", withExtension: "png"),
           let image = NSImage(contentsOf: url) {
            return image
        }
        return NSApp.applicationIconImage
    }
}

extension NSView {
    func pinToSuperview(_ insets: NSEdgeInsets = NSEdgeInsets(top: 0, left: 0, bottom: 0, right: 0)) {
        guard let superview else { return }
        translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            leadingAnchor.constraint(equalTo: superview.leadingAnchor, constant: insets.left),
            trailingAnchor.constraint(equalTo: superview.trailingAnchor, constant: -insets.right),
            topAnchor.constraint(equalTo: superview.topAnchor, constant: insets.top),
            bottomAnchor.constraint(equalTo: superview.bottomAnchor, constant: -insets.bottom),
        ])
    }
}
