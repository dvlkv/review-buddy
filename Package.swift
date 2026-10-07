// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "ReviewBuddy",
    platforms: [.macOS(.v26)],
    products: [
        .executable(name: "ReviewBuddy", targets: ["ReviewBuddy"]),
    ],
    targets: [
        .target(name: "ReviewBuddyCore"),
        .target(
            name: "ReviewBuddyUI",
            dependencies: ["ReviewBuddyCore"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .executableTarget(
            name: "ReviewBuddy",
            dependencies: ["ReviewBuddyCore", "ReviewBuddyUI"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .testTarget(
            name: "ReviewBuddyCoreTests",
            dependencies: ["ReviewBuddyCore"],
            resources: [.copy("Fixtures")]
        ),
        .testTarget(
            name: "ReviewBuddyUITests",
            dependencies: ["ReviewBuddyCore", "ReviewBuddyUI"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
    ]
)
