//! Ownership of the app's WebKit text-checking state.
//!
//! macOS delivers Text Replacement, autocorrect, spell checking, smart quotes,
//! smart dashes and link detection through one unified text-checking pass. The
//! webview's element attributes can only turn that whole pass on or off, so the
//! individual substitutions are selected here instead: WebKit's text checker
//! reads its state from the app's own `NSUserDefaults` the first time it runs,
//! and caches it for the life of the process.

/// Every WebKit text-checking default Sticky owns, with the value it must hold.
///
/// Each key is written explicitly. Unwritten keys are seeded from the user's
/// global `NSSpellChecker` preferences, which would make note behavior vary
/// from machine to machine.
#[cfg(target_os = "macos")]
const TEXT_CHECKING_DEFAULTS: [(&str, bool); 7] = [
    // The reason this module exists: shortcuts from System Settings → Keyboard
    // → Text Replacements expand inside notes.
    ("WebAutomaticTextReplacementEnabled", true),
    ("WebAutomaticLinkDetectionEnabled", true),
    // Nothing else may rewrite what was typed.
    ("WebAutomaticSpellingCorrectionEnabled", false),
    ("WebAutomaticQuoteSubstitutionEnabled", false),
    ("WebAutomaticDashSubstitutionEnabled", false),
    // Notes stay free of spelling and grammar marks.
    ("WebContinuousSpellCheckingEnabled", false),
    ("WebGrammarCheckingEnabled", false),
];

/// Applies Sticky's text-checking preferences.
///
/// Must run before any webview exists, because WebKit reads these once.
pub(crate) fn configure_macos_text_checking() {
    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::{NSString, NSUserDefaults};

        let defaults = NSUserDefaults::standardUserDefaults();
        for (key, enabled) in TEXT_CHECKING_DEFAULTS {
            // The application domain is used deliberately: registered defaults
            // would lose to the ones WebKit seeds from NSSpellChecker.
            defaults.setBool_forKey(enabled, &NSString::from_str(key));
        }
    }
}
