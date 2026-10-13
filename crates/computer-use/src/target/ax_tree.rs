//! Bounded accessibility tree, element lookup, and background capability (CU-05).

use super::*;

// ---------- bounded accessibility tree (CU-05) ----------

/// The bounds every accessibility-tree read runs with. The backend uses
/// exactly these on the initial read and on every revision re-check, so an
/// unchanged tree always yields the same revision; host-chosen bounds
/// would break that agreement, so they are fixed here instead of being a
/// per-call knob.
pub const AX_TREE_BOUNDS: AxTreeBounds = AxTreeBounds {
    max_depth: 24,
    max_nodes: 4096,
    max_text: 200,
    max_read_ms: 10_000,
};

/// Hard limits of one bounded tree read: depth, emitted nodes, per-string
/// text length and wall-clock read time. Depth, node and text truncation
/// are deterministic for an unchanged tree and are flagged on the read.
/// Exceeding the time budget fails the read instead, because a timed-out
/// partial tree can support neither a stable revision nor a provable
/// lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxTreeBounds {
    /// Deepest generation visited below the window root (root depth = 0).
    pub max_depth: u32,
    /// Most nodes emitted by one read, root included.
    pub max_nodes: u32,
    /// Longest kept name, in chars; longer names are cut and flagged.
    pub max_text: usize,
    /// Wall-clock budget of one read in milliseconds.
    pub max_read_ms: u64,
}

impl AxTreeBounds {
    /// Structural validity: every bound nonzero and within the hard
    /// ceilings, so a read is always finite.
    pub fn validate(&self) -> Result<(), TargetError> {
        const MAX_DEPTH: u32 = 64;
        const MAX_NODES: u32 = 8192;
        const MAX_TEXT: usize = 1024;
        const MAX_READ_MS: u64 = 30_000;
        if self.max_depth == 0
            || self.max_depth > MAX_DEPTH
            || self.max_nodes == 0
            || self.max_nodes > MAX_NODES
            || self.max_text == 0
            || self.max_text > MAX_TEXT
            || self.max_read_ms == 0
            || self.max_read_ms > MAX_READ_MS
        {
            return Err(TargetError::Invalid(
                "accessibility tree bounds out of range",
            ));
        }
        Ok(())
    }

    pub fn max_read(&self) -> Duration {
        Duration::from_millis(self.max_read_ms)
    }
}

/// One element of a bounded tree read. Names are readable labels only
/// (AXTitle, falling back to AXDescription); element content (AXValue) is
/// never read, so reading a tree never scoops up passwords, tokens or
/// document text. Secure-input roles are emitted with role, state, frame
/// and actions only; even their name is not read.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AxTreeNode {
    /// Child-index path from the window root within this read (root: []):
    /// host-side bookkeeping that lets a later dispatch re-resolve the
    /// element after the tree-revision check proved the tree unchanged.
    /// Never a cross-observation identity.
    pub path: Vec<u32>,
    pub depth: u32,
    pub role: String,
    /// Readable name, truncated to [AxTreeBounds::max_text]; None when the
    /// element has no usable name or is a secure-input role.
    pub name: Option<String>,
    pub name_truncated: bool,
    /// The name read itself failed (neither AXTitle nor AXDescription
    /// answered): None here is then NOT evidence of absence, and a
    /// name-based lookup over this read is unprovable (see
    /// [lookup_element]). Also flags the read via
    /// [AxTreeTruncation::names].
    pub name_unreadable: bool,
    /// Readable state; None when the element does not expose it or the
    /// attribute could not be read (never a claim of false).
    pub enabled: Option<bool>,
    pub focused: Option<bool>,
    /// Window-local frame [x, y, w, h] in points; None when the element
    /// exposes no position/size. Window-local, so a pure window move
    /// changes neither the emitted nodes nor the revision.
    pub frame: Option<[f64; 4]>,
    /// Actions the element reports (AXPress, …).
    pub actions: Vec<String>,
    /// Children the application reported, whether or not they were
    /// visited within the bounds.
    pub children: u32,
    /// Children exist but were not visited because of the depth bound.
    pub depth_limited: bool,
}

/// Where one read is less than the full truth. The depth, node and text
/// bounds cut a read short deterministically (the time budget never
/// truncates; it fails the read); `names` records that at least one
/// element's name could not be read — not a bound, but the same class of
/// incompleteness: a name-based lookup over the read is unprovable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct AxTreeTruncation {
    pub depth: bool,
    pub nodes: bool,
    pub text: bool,
    /// At least one element's name read failed; the emitted None names
    /// are not all proven absences.
    pub names: bool,
}

impl AxTreeTruncation {
    pub fn any(&self) -> bool {
        self.depth || self.nodes || self.text || self.names
    }
}

/// One bounded read of a window's accessibility tree. nodes is preorder
/// and nodes[0] is always the window root, so a successful read is never
/// empty; an application exposing no elements beyond the window frame
/// yields exactly the root node. Read failures (missing permission,
/// unresponsive application, dead window) are [TargetError]s, never an
/// empty or partial read, so "no elements" is always an affirmative
/// answer rather than a disguised failure.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AxTreeRead {
    pub window: WindowIdentity,
    /// Revision of this read, from [ax_tree_revision]. The host registers
    /// it as the [ObservationKind::AxTree] revision; element handles
    /// issued from the observation stay valid only while the backend's
    /// current read fingerprints identically.
    pub tree_revision: u64,
    pub nodes: Vec<AxTreeNode>,
    pub truncation: AxTreeTruncation,
}

/// Deterministic revision of one bounded read: FNV-1a over the canonical
/// node stream plus the truncation flags. Same tree and same bounds yield
/// the same revision; an emitted difference (role, name, state, frame,
/// actions, child counts, truncation) changes the input stream. The
/// fingerprint is a 64-bit non-cryptographic hash: it is a cheap change
/// signal, not a proof — a colliding edit would go unnoticed, which is an
/// accepted limitation of the fixed-size digest. Frames are
/// window-local, so a pure window move does not advance the revision
/// (CU-04 move tolerance) while a resize or layout change does.
pub fn ax_tree_revision(nodes: &[AxTreeNode], truncation: AxTreeTruncation) -> u64 {
    /// FNV-1a with explicit tags and length prefixes, so optional and
    /// variable-length fields stay unambiguous.
    struct Hash(u64);
    impl Hash {
        fn bytes(&mut self, bytes: &[u8]) {
            for &byte in bytes {
                self.0 ^= u64::from(byte);
                self.0 = self.0.wrapping_mul(0x0000_0100_0000_01B3);
            }
        }
        fn number(&mut self, value: u64) {
            self.bytes(&value.to_le_bytes());
        }
        fn text(&mut self, text: &str) {
            self.number(text.len() as u64);
            self.bytes(text.as_bytes());
        }
        fn flag(&mut self, value: bool) {
            self.bytes(&[u8::from(value)]);
        }
    }
    let mut hash = Hash(0xcbf2_9ce4_8422_2325);
    for node in nodes {
        hash.number(u64::from(node.depth));
        hash.text(&node.role);
        hash.flag(node.name.is_some());
        if let Some(name) = &node.name {
            hash.text(name);
        }
        hash.flag(node.name_truncated);
        hash.flag(node.name_unreadable);
        for state in [node.enabled, node.focused] {
            hash.bytes(&[match state {
                None => 0,
                Some(false) => 1,
                Some(true) => 2,
            }]);
        }
        hash.flag(node.frame.is_some());
        if let Some(frame) = node.frame {
            for component in frame {
                hash.number(component.to_bits());
            }
        }
        hash.number(node.actions.len() as u64);
        for action in &node.actions {
            hash.text(action);
        }
        hash.number(u64::from(node.children));
        hash.flag(node.depth_limited);
    }
    hash.number(nodes.len() as u64);
    hash.flag(truncation.depth);
    hash.flag(truncation.nodes);
    hash.flag(truncation.text);
    hash.flag(truncation.names);
    hash.0
}

/// Element lookup criteria: exact role and/or case-insensitive name
/// substring. At least one is required.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElementQuery {
    pub role: Option<String>,
    pub name: Option<String>,
}

impl ElementQuery {
    pub fn new(role: Option<&str>, name: Option<&str>) -> Result<Self, TargetError> {
        let role = role
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let name = name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if role.is_none() && name.is_none() {
            return Err(TargetError::Invalid("element query needs a role or a name"));
        }
        Ok(Self { role, name })
    }

    fn matches(&self, node: &AxTreeNode) -> bool {
        if let Some(role) = &self.role {
            if &node.role != role {
                return false;
            }
        }
        if let Some(name) = &self.name {
            let Some(candidate) = &node.name else {
                return false;
            };
            if !candidate.to_lowercase().contains(&name.to_lowercase()) {
                return false;
            }
        }
        true
    }
}

/// The honest outcome of one lookup over a bounded read. A lookup never
/// silently picks the first of several matches, and never issues
/// uniqueness it cannot prove.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElementLookup {
    /// Exactly one match in a complete (untruncated) tree: the host may
    /// issue an element handle for nodes[index].
    Unique { index: usize },
    /// Zero matches. tree_truncated marks whether the unread part of the
    /// tree (or an unreadable name, for a name query) could have hidden a
    /// match, so "not found" is never confused with "not read".
    NoMatch { tree_truncated: bool },
    /// More than one match; matches counts them within the read (a lower
    /// bound when the tree was truncated). Refine the query.
    Ambiguous {
        matches: usize,
        tree_truncated: bool,
    },
    /// Exactly one match but an unprovable read: an untraversed subtree
    /// (or an unreadable name, for a name query) could hold another
    /// match, so uniqueness is unproven and the host must not issue a
    /// handle from it.
    UnprovenUnique { index: usize },
}

/// Locate elements of one bounded read by role and/or name. Provability
/// is exact: depth/node truncation hides whole subtrees from every
/// query, while text truncation and unreadable names only undermine
/// queries that read names — a role-only query over a structurally
/// complete read stays provable even when some names were cut or
/// unreadable.
pub fn lookup_element(read: &AxTreeRead, query: &ElementQuery) -> ElementLookup {
    let truncated = read.truncation.any();
    let provable = !(read.truncation.depth || read.truncation.nodes)
        && (query.name.is_none() || !(read.truncation.text || read.truncation.names));
    let matches: Vec<usize> = read
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| query.matches(node))
        .map(|(index, _)| index)
        .collect();
    match matches.len() {
        0 => ElementLookup::NoMatch {
            tree_truncated: truncated,
        },
        1 if !provable => ElementLookup::UnprovenUnique { index: matches[0] },
        1 => ElementLookup::Unique { index: matches[0] },
        n => ElementLookup::Ambiguous {
            matches: n,
            tree_truncated: truncated,
        },
    }
}

/// Action vocabulary for background-capability checks. Payloads (text, keys,
/// points) ride on the validated handles downstream; this enum names what may
/// run in native background at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeActionKind {
    /// Window image capture (ScreenCaptureKit).
    CaptureWindow,
    /// Read the window accessibility tree/values.
    AxRead,
    /// AXSetValue / selected-text insertion on an element.
    AxSemanticWrite,
    /// AXPress on an element.
    AxPress,
    /// Targeted text/key events (CGEventPostToPid).
    TargetedKeys,
    /// Targeted menu shortcut such as Cmd+S.
    MenuShortcut,
    /// Pressing menu-bar items via AX.
    MenubarPress,
    /// Pointer click/move/drag/scroll events.
    Pointer,
    /// Move the window (AXPosition).
    WindowMove,
    /// Resize the window (AXSize).
    WindowResize,
}

/// Measured background support. Unverified means CU-01 could not prove the
/// action on that family; it is rejected exactly like Unsupported until a
/// real measurement promotes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    Supported,
    Unsupported,
    Unverified,
}

/// The CU-01 measured matrix (docs/plan/cu-01-background-feasibility.md).
/// minimized changes support in two measured ways: AppKit targeted keys are
/// silently dropped while minimized, and window move/resize was only measured
/// in background/occluded states.
pub fn background_capability(
    family: AppFamily,
    action: NativeActionKind,
    minimized: bool,
) -> Capability {
    use AppFamily::*;
    use Capability::*;
    use NativeActionKind::*;
    match action {
        CaptureWindow | AxRead => Supported,
        Pointer => Unsupported,
        AxSemanticWrite => match family {
            Appkit => Supported,
            Chromium => Unsupported,
            Browser => Unverified,
        },
        // CU-01 proved AXPress on web pages (JS/navigation). On AppKit only
        // AXSetValue / AXSelectedText writes were proven, and on Chromium the
        // press effect was undetermined.
        AxPress => match family {
            Appkit | Chromium => Unverified,
            Browser => Supported,
        },
        TargetedKeys => match family {
            Appkit => {
                if minimized {
                    Unsupported
                } else {
                    Supported
                }
            }
            Chromium | Browser => Unsupported,
        },
        MenuShortcut | MenubarPress => match family {
            Appkit => Unsupported,
            Chromium | Browser => Unverified,
        },
        // Minimized-state window move/resize was not measured in CU-01.
        WindowMove => match family {
            Appkit => {
                if minimized {
                    Unverified
                } else {
                    Supported
                }
            }
            Chromium | Browser => Unsupported,
        },
        WindowResize => {
            if minimized {
                Unverified
            } else {
                Supported
            }
        }
    }
}

/// Gate before any native-background dispatch. Only Supported passes; there
/// is no fallback to global or foreground input.
pub fn require_background(
    family: AppFamily,
    action: NativeActionKind,
    minimized: bool,
) -> Result<(), TargetError> {
    match background_capability(family, action, minimized) {
        Capability::Supported => Ok(()),
        Capability::Unsupported | Capability::Unverified => Err(TargetError::BackgroundUnsupported),
    }
}
