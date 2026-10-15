//! Assets: the compositions and image documents of a studio project, and how other engines refer
//! to them (`studio://comp/<id>`, `studio://image/<id>`).

use serde::{Deserialize, Serialize};

/// A project-wide id for a composition or an image document. Stable for the life of the project,
/// whatever id the owning engine uses internally.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AssetId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    /// A motion composition, owned by the motion engine (EffectCraft).
    Composition,
    /// A layered image document, owned by the image engine (PhotoCraft).
    Image,
}

impl AssetKind {
    pub fn scheme_segment(self) -> &'static str {
        match self {
            AssetKind::Composition => "comp",
            AssetKind::Image => "image",
        }
    }
}

/// One asset and where its pieces live in each engine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub id: AssetId,
    pub kind: AssetKind,
    pub name: String,
    /// The motion engine's composition item id (compositions only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comp: Option<u64>,
    /// The timeline item that shows this asset, once it has been placed on a sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeline_item: Option<u64>,
    /// The motion engine's footage item that shows this image inside compositions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub motion_footage: Option<u64>,
}

pub const SCHEME: &str = "studio://";

/// `studio://comp/7`
pub fn uri(kind: AssetKind, id: AssetId) -> String {
    format!("{SCHEME}{}/{}", kind.scheme_segment(), id.0)
}

/// The asset a `studio://…` reference names, if it is one.
pub fn parse_uri(s: &str) -> Option<(AssetKind, AssetId)> {
    let rest = s.strip_prefix(SCHEME)?;
    let (kind, id) = rest.split_once('/')?;
    let kind = match kind {
        "comp" => AssetKind::Composition,
        "image" => AssetKind::Image,
        _ => return None,
    };
    Some((kind, AssetId(id.parse().ok()?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_round_trip() {
        for (k, i) in [(AssetKind::Composition, 7), (AssetKind::Image, 12)] {
            assert_eq!(parse_uri(&uri(k, AssetId(i))), Some((k, AssetId(i))));
        }
        assert_eq!(parse_uri("studio://comp/x"), None);
        assert_eq!(parse_uri("file:///a.png"), None);
        assert_eq!(parse_uri("studio://sound/1"), None);
    }
}
