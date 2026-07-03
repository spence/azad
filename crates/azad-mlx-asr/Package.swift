// swift-tools-version:6.2
import PackageDescription

let package = Package(
    name: "AzadMlxAsr",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "azad-mlx-asr", targets: ["AzadMlxAsr"])
    ],
    dependencies: [
        .package(url: "https://github.com/ml-explore/mlx-swift.git", exact: "0.31.3"),
        .package(url: "https://github.com/ml-explore/mlx-swift-lm.git", exact: "3.31.3"),
        .package(
            url: "https://github.com/Blaizzy/mlx-audio-swift.git",
            revision: "0ea78a5a6fe9faf3b7f652c579f957a663b43466"
        )
    ],
    targets: [
        .executableTarget(
            name: "AzadMlxAsr",
            dependencies: [
                .product(name: "MLX", package: "mlx-swift"),
                .product(name: "MLXLMCommon", package: "mlx-swift-lm"),
                .product(name: "MLXAudioSTT", package: "mlx-audio-swift")
            ]
        )
    ]
)
