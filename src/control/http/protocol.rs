use serde::Deserialize;

use egui::epaint::{ClippedPrimitive, Primitive};

/// One browser input message. Coordinates are CSS pixels (egui points).
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputEvent {
    Hello {
        width: f32,
        height: f32,
        dpr: f32,
    },
    Resize {
        width: f32,
        height: f32,
        dpr: f32,
    },
    PointerMove {
        x: f32,
        y: f32,
    },
    PointerButton {
        button: PointerButton,
        pressed: bool,
        x: f32,
        y: f32,
    },
    Scroll {
        dx: f32,
        dy: f32,
        #[serde(default)]
        alt: bool,
        #[serde(default)]
        ctrl: bool,
        #[serde(default)]
        shift: bool,
        #[serde(default)]
        command: bool,
    },
    Key {
        key: String,
        pressed: bool,
        repeat: bool,
        alt: bool,
        ctrl: bool,
        shift: bool,
        command: bool,
    },
    Text {
        text: String,
    },
    Focus {
        focused: bool,
    },
    /// The socket closed: release whatever this client was holding.
    ClientGone,
    /// Send the full texture set with the next frame (new client or a lag).
    Keyframe,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointerButton {
    Primary,
    Secondary,
    Middle,
}

impl PointerButton {
    pub fn to_egui(self) -> egui::PointerButton {
        return match self {
            PointerButton::Primary => egui::PointerButton::Primary,
            PointerButton::Secondary => egui::PointerButton::Secondary,
            PointerButton::Middle => egui::PointerButton::Middle,
        };
    }
}

/// A browser frame, encoded by [`encode_frame`].
pub type Frame = axum::body::Bytes;

const MAGIC: u32 = 0x4d56_5752;
const HEADER_LEN: usize = 28;
const TEXTURE_HEADER_LEN: usize = 20;
const MESH_HEADER_LEN: usize = 28;

/// Everything the browser needs to replace its texture: a full image or a patch.
pub struct TextureUpload {
    pub id: u32,
    /// Full image, not a patch. A patch at (0,0) must not be mistaken for one.
    pub full: bool,
    pub pos: [u16; 2],
    pub size: [u16; 2],
    pub pixels: Vec<u8>,
}

pub fn texture_key(id: egui::TextureId) -> u32 {
    return match id {
        egui::TextureId::Managed(index) => index as u32,
        egui::TextureId::User(index) => 0x8000_0000 | (index as u32 & 0x7fff_ffff),
    };
}

/// Encode tessellated egui meshes plus texture uploads into one binary frame.
pub fn encode_frame(primitives: &[ClippedPrimitive], textures: &[TextureUpload], free: &[u32]) -> Frame {
    let mut indices: Vec<u32> = Vec::new();
    let mut vertices: Vec<u8> = Vec::new();
    let mut meshes: Vec<(u32, [f32; 4], u32, u32)> = Vec::new();

    for clipped in primitives {
        let Primitive::Mesh(mesh) = &clipped.primitive else {
            continue;
        };
        let base = (vertices.len() / VERTEX_LEN) as u32;
        let first_index = indices.len() as u32;
        indices.extend(mesh.indices.iter().map(|index| index + base));
        for vertex in &mesh.vertices {
            vertices.extend_from_slice(&vertex.pos.x.to_le_bytes());
            vertices.extend_from_slice(&vertex.pos.y.to_le_bytes());
            vertices.extend_from_slice(&vertex.uv.x.to_le_bytes());
            vertices.extend_from_slice(&vertex.uv.y.to_le_bytes());
            vertices.extend_from_slice(&[vertex.color.r(), vertex.color.g(), vertex.color.b(), vertex.color.a()]);
        }
        meshes.push((
            texture_key(mesh.texture_id),
            [
                clipped.clip_rect.min.x,
                clipped.clip_rect.min.y,
                clipped.clip_rect.max.x,
                clipped.clip_rect.max.y,
            ],
            first_index * 4,
            mesh.indices.len() as u32,
        ));
    }

    let index_bytes = indices.len() * 4;
    let vertex_bytes = vertices.len();
    let texture_bytes: usize = textures.iter().map(|texture| texture.pixels.len()).sum();
    let mut out = Vec::with_capacity(
        HEADER_LEN
            + textures.len() * TEXTURE_HEADER_LEN
            + meshes.len() * MESH_HEADER_LEN
            + free.len() * 4
            + index_bytes
            + vertex_bytes
            + texture_bytes,
    );

    out.extend_from_slice(&MAGIC.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(textures.len() as u16).to_le_bytes());
    out.extend_from_slice(&(meshes.len() as u16).to_le_bytes());
    out.extend_from_slice(&(free.len() as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(index_bytes as u32).to_le_bytes());
    out.extend_from_slice(&(vertex_bytes as u32).to_le_bytes());
    out.extend_from_slice(&(texture_bytes as u32).to_le_bytes());

    let mut texture_offset = 0u32;
    for texture in textures {
        out.extend_from_slice(&texture.id.to_le_bytes());
        out.extend_from_slice(&texture.pos[0].to_le_bytes());
        out.extend_from_slice(&texture.pos[1].to_le_bytes());
        out.extend_from_slice(&texture.size[0].to_le_bytes());
        out.extend_from_slice(&texture.size[1].to_le_bytes());
        out.extend_from_slice(&(texture.full as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&texture_offset.to_le_bytes());
        texture_offset += texture.pixels.len() as u32;
    }
    for (texture, clip, index_offset, index_count) in &meshes {
        out.extend_from_slice(&texture.to_le_bytes());
        for value in clip {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&index_offset.to_le_bytes());
        out.extend_from_slice(&index_count.to_le_bytes());
    }
    for id in free {
        out.extend_from_slice(&id.to_le_bytes());
    }
    for index in &indices {
        out.extend_from_slice(&index.to_le_bytes());
    }
    out.extend_from_slice(&vertices);
    for texture in textures {
        out.extend_from_slice(&texture.pixels);
    }

    return out.into();
}

const VERTEX_LEN: usize = 20;

/// The web client, embedded so the binary has no asset directory.
pub const INDEX_HTML: &str = include_str!("assets/index.html");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_header_matches_the_client_layout() {
        let frame = encode_frame(&[], &[], &[]);
        assert_eq!(&frame[0..4], &MAGIC.to_le_bytes());
        assert_eq!(frame.len(), HEADER_LEN);
        let index_bytes = u32::from_le_bytes(frame[16..20].try_into().unwrap());
        let vertex_bytes = u32::from_le_bytes(frame[20..24].try_into().unwrap());
        let texture_bytes = u32::from_le_bytes(frame[24..28].try_into().unwrap());
        assert_eq!((index_bytes, vertex_bytes, texture_bytes), (0, 0, 0));
    }

    #[test]
    fn a_mesh_encodes_absolute_indices() {
        let mut mesh = egui::Mesh::default();
        mesh.vertices.push(egui::epaint::Vertex {
            pos: egui::pos2(1.0, 2.0),
            uv: egui::pos2(0.0, 0.0),
            color: egui::Color32::WHITE,
        });
        mesh.indices.push(0);
        let primitives = vec![ClippedPrimitive {
            clip_rect: egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(10.0, 10.0)),
            primitive: Primitive::Mesh(mesh),
        }];
        let frame = encode_frame(&primitives, &[], &[]);
        let vertex_bytes = u32::from_le_bytes(frame[20..24].try_into().unwrap());
        assert_eq!(vertex_bytes as usize, VERTEX_LEN);
        let offsets = HEADER_LEN + MESH_HEADER_LEN;
        assert_eq!(&frame[offsets..offsets + 4], &0u32.to_le_bytes());
    }
}
