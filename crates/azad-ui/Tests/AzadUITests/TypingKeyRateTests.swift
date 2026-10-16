import AppKit
import XCTest
@testable import AzadUI

private var capturedEvents: [String] = []

private func captureEvent(_ json: UnsafePointer<CChar>?) {
    if let json { capturedEvents.append(String(cString: json)) }
}

final class TypingKeyRateTests: XCTestCase {
    private func fixture() throws -> [String: Any] {
        let path = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("azad/docs/typing-key-rate.fixture.json")
        return try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: path)) as? [String: Any])
    }

    private func model(method: Int, rate: Int, tab: SettingsTab = .general) -> SettingsViewModel {
        SettingsViewModel(
            selectedTab: tab, accessibilityStatus: .granted, microphoneStatus: .granted,
            runOnStartupEnabled: false, startupListenModeIndex: 2, activationLevel: 0,
            historyEnabled: true, pasteMethodIndex: method, typingKeyRate: rate,
            autoSubmitIndex: 0, overlayPositionIndex: 0, appendTrailingSpaceOnPaste: true,
            deduplicateWordsOnPaste: false, convertNumberWordsOnPaste: false,
            convertSpokenEmojiOnPaste: false, lowercaseExceptUppercaseWordsOnPaste: false,
            removeHesitationsOnPaste: true, listenModifiers: 4, debugStatsEnabled: false,
            metricsText: "", model: ModelPack(
                id: "test", welcomeName: "Test", settingsName: "Test", pageUrl: "",
                description: "", sizeLabel: "", status: .ready, downloadPaused: false,
                progressPct: 100, bytesDoneLabel: "", bytesTotalLabel: "", errorMessage: ""
            ), removedWords: [], connectors: [], buildInfo: "layout test"
        )
    }

    private func descendants(_ view: NSView) -> [NSView] {
        [view] + view.subviews.flatMap(descendants)
    }

    func testGeneralLayoutAndConditionalControlAtEveryDetent() throws {
        XCTAssertTrue(Thread.isMainThread)
        _ = NSApplication.shared
        let fixture = try fixture()
        let rates = try XCTUnwrap(fixture["sliderRates"] as? [Int])
        let visible = try XCTUnwrap(fixture["visibleForPasteMethodIndices"] as? [Int])
        XCTAssertEqual(SettingsWindowController.typingKeyRates, rates)
        XCTAssertEqual(fixture["default"] as? Int, 0)
        let baseline = SettingsWindowController()
        baseline.prepare(model: model(method: 0, rate: 0, tab: .text))
        let baselineRoot = try XCTUnwrap(baseline.window?.contentView)
        baselineRoot.layoutSubtreeIfNeeded()
        let baselineRows = descendants(baselineRoot).compactMap { $0 as? FormRow }
        let baselineAmbiguity = try XCTUnwrap(baselineRows.first).hasAmbiguousLayout
        XCTAssertTrue(baselineAmbiguity)
        for method in 0...2 {
            for (position, rate) in rates.enumerated() {
                let controller = SettingsWindowController()
                controller.prepare(model: model(method: method, rate: rate))
                let window = try XCTUnwrap(controller.window)
                let root = try XCTUnwrap(window.contentView)
                root.layoutSubtreeIfNeeded()
                XCTAssertFalse(window.isVisible)
                XCTAssertEqual(root.bounds.size, NSSize(width: 656, height: 391))
                let views = descendants(root)
                for row in views.compactMap({ $0 as? FormRow }) {
                    XCTAssertEqual(row.hasAmbiguousLayout, baselineAmbiguity)
                    XCTAssertTrue(root.bounds.contains(row.convert(row.bounds, to: root)))
                }
                let keySliders = views.compactMap { $0 as? NSSlider }
                    .filter { $0.accessibilityLabel() == "Key rate" }
                XCTAssertEqual(keySliders.count, visible.contains(method) ? 1 : 0)
                let popup = try XCTUnwrap(views.compactMap { $0 as? NSPopUpButton }
                    .first { $0.itemTitles == ["Paste", "Type", "Type and copy"] })
                XCTAssertEqual(popup.frame.width, 145, accuracy: 0.5)
                XCTAssertLessThanOrEqual(popup.fittingSize.width, popup.frame.width + 0.5)
                for label in views.compactMap({ $0 as? NSTextField }) where label.stringValue == "Overlay position" {
                    XCTAssertLessThanOrEqual(label.fittingSize.width, label.frame.width + 0.5)
                }
                if let slider = keySliders.first {
                    XCTAssertEqual(slider.integerValue, position)
                    XCTAssertFalse(slider.isContinuous)
                    XCTAssertTrue(slider.allowsTickMarkValuesOnly)
                    XCTAssertEqual(slider.numberOfTickMarks, 6)
                    XCTAssertEqual(slider.frame.width, 72, accuracy: 0.5)
                    let stack = try XCTUnwrap(slider.superview as? NSStackView)
                    let frames = stack.arrangedSubviews.map { $0.convert($0.bounds, to: root) }
                    for frame in frames {
                        XCTAssertTrue(root.bounds.contains(frame), "clipped insert control: \(frame)")
                    }
                    for pair in zip(frames, frames.dropFirst()) {
                        XCTAssertLessThanOrEqual(pair.0.maxX, pair.1.minX + 0.5)
                    }
                    let readout = try XCTUnwrap(stack.arrangedSubviews.last as? NSTextField)
                    XCTAssertEqual(readout.stringValue, rate == 0 ? "Max" : "\(rate)/s")
                    XCTAssertLessThanOrEqual(readout.fittingSize.width, readout.frame.width + 0.5)
                    capturedEvents = []
                    azadUIRegisterCallback(captureEvent)
                    defer { azadUIRegisterCallback(nil) }
                    XCTAssertTrue(slider.sendAction(slider.action, to: slider.target))
                    let eventData = try XCTUnwrap(capturedEvents.last?.data(using: .utf8))
                    let event = try XCTUnwrap(JSONSerialization.jsonObject(with: eventData) as? [String: Any])
                    XCTAssertEqual(event["action"] as? String, "setTypingKeyRate")
                    XCTAssertEqual(event["index"] as? Int, rate)
                }
            }
        }
    }

    func testModelCopiesKeepRateAndPasteDoesNotEraseIt() throws {
        _ = NSApplication.shared
        let controller = SettingsWindowController()
        controller.prepare(model: model(method: 1, rate: 80))
        controller.updatePermissions(accessibility: .denied, microphone: .granted)
        XCTAssertEqual(controller.model?.typingKeyRate, 80)
        let root = try XCTUnwrap(controller.window?.contentView)
        let activation = try XCTUnwrap(descendants(root).compactMap { $0 as? NSSlider }
            .first { $0.maxValue == 100 })
        activation.integerValue = 25
        XCTAssertTrue(activation.sendAction(activation.action, to: activation.target))
        XCTAssertEqual(controller.model?.typingKeyRate, 80)
        XCTAssertEqual(controller.model?.activationLevel, 25)
        controller.prepare(model: model(method: 0, rate: 80))
        XCTAssertEqual(controller.model?.typingKeyRate, 80)
        controller.prepare(model: model(method: 2, rate: 80))
        XCTAssertEqual(descendants(root).compactMap { $0 as? NSSlider }
            .first { $0.accessibilityLabel() == "Key rate" }?.integerValue, 2)
    }

    func testOtherTabsKeepOriginalLabelWidthAndLeadingInset() throws {
        _ = NSApplication.shared
        let controller = SettingsWindowController()
        controller.prepare(model: model(method: 1, rate: 80, tab: .text))
        let root = try XCTUnwrap(controller.window?.contentView)
        root.layoutSubtreeIfNeeded()
        let label = try XCTUnwrap(descendants(root).compactMap { $0 as? NSTextField }
            .first { $0.stringValue == "Trailing space" })
        let aligned = label.alignmentRect(forFrame: label.frame)
        XCTAssertEqual(aligned.width, 150, accuracy: 0.5)
        let labelOrigin = try XCTUnwrap(label.superview).convert(aligned.origin, to: root)
        XCTAssertEqual(labelOrigin.x, 191, accuracy: 0.5)
        XCTAssertFalse(descendants(root).compactMap { $0 as? NSSlider }
            .contains { $0.accessibilityLabel() == "Key rate" })
    }
}
