import Foundation
import CoreServices
import ServiceManagement

if CommandLine.arguments.count > 2 && CommandLine.arguments[1] == "--helper" {
    let marker = URL(fileURLWithPath: CommandLine.arguments[2])
    let data = try JSONSerialization.data(withJSONObject: ["effectiveUid": geteuid(), "helperRan": true], options: [.sortedKeys])
    try data.write(to: marker, options: [.atomic])
    exit(0)
}

let marker = URL(fileURLWithPath: CommandLine.arguments[1])
let service = SMAppService.daemon(plistName: "com.bgasol.singshell.preflight.plist")
var report: [String: Any] = ["before": service.status.rawValue, "bundle": Bundle.main.bundlePath]
LSRegisterURL(Bundle.main.bundleURL as CFURL, true)
do {
    try service.register()
    report["registered"] = true
    report["after"] = service.status.rawValue
    if service.status == .enabled {
        for _ in 0..<50 {
            if let data = try? Data(contentsOf: marker), let worker = try? JSONSerialization.jsonObject(with: data) {
                report["worker"] = worker
                break
            }
            Thread.sleep(forTimeInterval: 0.1)
        }
    } else if service.status == .requiresApproval {
        report["manualApprovalRequired"] = true
    }
    try service.unregister()
    report["unregistered"] = true
} catch {
    report["error"] = String(describing: error)
    try? service.unregister()
}
let data = try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
print(String(decoding: data, as: UTF8.self))
if report["registered"] as? Bool != true { exit(1) }
