// swift-tools-version:6.2
import PackageDescription

let package = Package(
    name: "AzadUI",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "AzadUI", type: .dynamic, targets: ["AzadUI"]),
        .executable(name: "azad-ui-preview", targets: ["AzadUIPreview"]),
    ],
    targets: [
        .target(
            name: "AzadUI",
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .executableTarget(
            name: "AzadUIPreview",
            dependencies: ["AzadUI"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
    ]
)
