import AVFoundation
import Foundation

let permissionRequestButtonTag = 1

func performNativePermissionRequest(_ permission: String, completion: @escaping () -> Void) {
    if permission == "microphone" {
        AVCaptureDevice.requestAccess(for: .audio) { _ in
            DispatchQueue.main.async {
                completion()
            }
        }
        return
    }

    completion()
}
