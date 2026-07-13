// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "azad-apple-lm",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "azad-apple-lm", targets: ["AzadAppleLm"]),
    ],
    targets: [
        .executableTarget(
            name: "AzadAppleLm",
            path: "Sources/AzadAppleLm"
        ),
    ]
)
