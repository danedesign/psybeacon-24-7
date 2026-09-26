import Foundation

/// Translates macOS `NSEvent.keyCode` (Carbon-era hardware scancodes,
/// non-alphabetical and non-obvious — e.g. 'A' is 0x00 but 'S' is 0x01)
/// into Windows virtual-key codes, which `sidecar.rs` feeds straight into
/// `SendInput` on the host with no translation of its own.
///
/// Covers letters, digits, common editing/navigation keys, modifiers, and
/// F1–F12 — enough for practical remote-control typing and navigation.
/// Deliberately not exhaustive (keypad, media keys, ISO/JIS extra keys,
/// F13+ are missing): extend this table if a missing key turns out to
/// matter in practice, rather than guessing every entry up front against
/// keycodes that can't be verified from this machine (no macOS here to
/// confirm against a real `NSEvent`).
enum KeycodeMap {
    static func windowsVirtualKey(forMacKeyCode keyCode: UInt16) -> UInt16? {
        table[keyCode]
    }

    private static let table: [UInt16: UInt16] = [
        // Letters (macOS keyCode -> Windows VK, which equals ASCII uppercase)
        0x00: 0x41, // A
        0x01: 0x53, // S
        0x02: 0x44, // D
        0x03: 0x46, // F
        0x04: 0x48, // H
        0x05: 0x47, // G
        0x06: 0x5A, // Z
        0x07: 0x58, // X
        0x08: 0x43, // C
        0x09: 0x56, // V
        0x0B: 0x42, // B
        0x0C: 0x51, // Q
        0x0D: 0x57, // W
        0x0E: 0x45, // E
        0x0F: 0x52, // R
        0x10: 0x59, // Y
        0x11: 0x54, // T
        0x1F: 0x4F, // O
        0x20: 0x55, // U
        0x22: 0x49, // I
        0x23: 0x50, // P
        0x25: 0x4C, // L
        0x26: 0x4A, // J
        0x28: 0x4B, // K
        0x2D: 0x4E, // N
        0x2E: 0x4D, // M

        // Digits (macOS keyCode -> Windows VK, which equals ASCII digit)
        0x12: 0x31, // 1
        0x13: 0x32, // 2
        0x14: 0x33, // 3
        0x15: 0x34, // 4
        0x16: 0x36, // 6
        0x17: 0x35, // 5
        0x19: 0x39, // 9
        0x1A: 0x37, // 7
        0x1C: 0x38, // 8
        0x1D: 0x30, // 0

        // Whitespace / editing
        0x24: 0x0D, // Return -> VK_RETURN
        0x30: 0x09, // Tab -> VK_TAB
        0x31: 0x20, // Space -> VK_SPACE
        0x33: 0x08, // Delete (backspace) -> VK_BACK
        0x75: 0x2E, // Forward Delete -> VK_DELETE
        0x35: 0x1B, // Escape -> VK_ESCAPE

        // Modifiers
        0x37: 0x5B, // Command -> VK_LWIN
        0x38: 0x10, // Shift -> VK_SHIFT
        0x3A: 0x12, // Option -> VK_MENU (Alt)
        0x3B: 0x11, // Control -> VK_CONTROL
        0x3C: 0x10, // Right Shift -> VK_SHIFT
        0x3D: 0x12, // Right Option -> VK_MENU
        0x3E: 0x11, // Right Control -> VK_CONTROL

        // Arrows
        0x7B: 0x25, // Left -> VK_LEFT
        0x7C: 0x27, // Right -> VK_RIGHT
        0x7D: 0x28, // Down -> VK_DOWN
        0x7E: 0x26, // Up -> VK_UP

        // Navigation
        0x73: 0x24, // Home -> VK_HOME
        0x77: 0x23, // End -> VK_END
        0x74: 0x21, // Page Up -> VK_PRIOR
        0x79: 0x22, // Page Down -> VK_NEXT

        // Function keys
        0x7A: 0x70, // F1
        0x78: 0x71, // F2
        0x63: 0x72, // F3
        0x76: 0x73, // F4
        0x60: 0x74, // F5
        0x61: 0x75, // F6
        0x62: 0x76, // F7
        0x64: 0x77, // F8
        0x65: 0x78, // F9
        0x6D: 0x79, // F10
        0x67: 0x7A, // F11
        0x6F: 0x7B, // F12
    ]
}
