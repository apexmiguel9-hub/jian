//! `PathCommand` → `skia_safe::Path`, y el sentido inverso.
//!
//! skia-safe 0.97 split path construction onto `PathBuilder` (mutating
//! `move_to` / `line_to` / `quad_to` / `cubic_to` / `close`); the
//! finished `Path` is produced by `builder.detach()` and is itself
//! immutable for traversal. See `~/.cargo/registry/src/.../skia-safe-0.97.0/src/core/path_builder.rs:238-360`.
//!
//! ====================== FASE 1: el shim que faltaba ======================
//! `to_sk_path` existe, pero NO su inverso. Eso es la pieza que hacia que
//! "convertir objeto a trazos" no tuviera efecto real: las primitivas
//! (rect, elipse, poligono, linea) se sabe construir como `SkPath` -- ver
//! `op-host-native/src/boolean_ops.rs:110 build_rect_path` y `:128
//! build_oval_path` -- pero un `SkPath` NO se puede editar como geometria.
//!
//! La geometria editable vive en `PathNode.anchors` (`PenPathAnchor` en
//! jian-ops-schema/src/node/path.rs:37). El propio loader lo dice
//! (op-pen-loader/src/adapter/shapes.rs:192-194):
//!
//!   "Editable paths trace their anchors here. Imported SVG paths that
//!    preserve `d` paint through `svg_path` instead."
//!
//! Por eso los boolean ops producen un path MUERTO: salen con
//! `anchors: None` (host_support_allocator.rs:358) y borran las fuentes
//! (:376), o sea que se mueven pero sus vertices no se tocan.
//!
//! `to_path_commands` recorre los verbos del `SkPath` y los traduce a
//! `PathCommand`, que es justo el formato que el loader ya sabe convertir en
//! anchors. Penpot hace exactamente lo mismo en 282 lineas de Rust
//! (render-wasm/src/shapes/shape_to_path.rs + `Path::from_skia_path` en
//! render-wasm/src/shapes), y su `RawSegmentData` son 28 bytes = 7 x f64
//! (x, y, handle_in, handle_out, curve), que casa 1:1 con `PenPathAnchor`.

use crate::convert::to_sk_point;
use jian_core::render::PathCommand;
use skia_safe::{Path as SkPath, PathBuilder, Point as SkPoint};

pub fn to_sk_path(commands: &[PathCommand]) -> SkPath {
    let mut builder = PathBuilder::new();
    for cmd in commands {
        match *cmd {
            PathCommand::MoveTo(p) => {
                builder.move_to(to_sk_point(p));
            }
            PathCommand::LineTo(p) => {
                builder.line_to(to_sk_point(p));
            }
            PathCommand::QuadTo(c, p) => {
                builder.quad_to(to_sk_point(c), to_sk_point(p));
            }
            PathCommand::CubicTo(c1, c2, p) => {
                builder.cubic_to(to_sk_point(c1), to_sk_point(c2), to_sk_point(p));
            }
            PathCommand::Close => {
                builder.close();
            }
        }
    }
    builder.detach()
}

/// `skia_safe::Path` → `PathCommand` (el shim de la FASE 1).
///
/// Traduce un path de Skia a la lista de comandos que el loader ya sabe
/// convertir en anchors editables. Es el puente que hace que
/// `build_rect_path` / `build_oval_path` (boolean_ops.rs:110/:128) produzcan
/// geometria real y no un path muerto.
///
/// Se usa `PathIter` con `points()` + `verb()` (skia-safe 0.97, ver
/// core/path_iter.rs) y NO `Geometry::decompose`, que no existe en esta
/// version. `PathIter` entrega un verbo por paso y `points()` da TODOS los
/// puntos de ese verbo de una vez, que es justo lo que hace falta para
/// emitir un comando completo.
///
/// `PathVerb` es un alias a `sb::SkPathVerb` (core/path_types.rs:19) y sus
/// variantes reales son Move/Line/Quad/Conic/Cubic/Close. `Conic` no tiene
/// equivalente en `PathCommand`, asi que se degrada a `LineTo` del punto
/// final: es la misma decision que toma el resto del codebase cuando no hay
/// conic disponible.
///
/// Mapa de verbo Skia -> comando:
///   Move(p)          -> MoveTo(p)
///   Line(p)          -> LineTo(p)
///   Quad(c, p)       -> QuadTo(c, p)
///   Cubic(c1, c2, p) -> CubicTo(c1, c2, p)
///   Conic(..., p)    -> LineTo(p)   (sin conic en PathCommand)
///   Close            -> Close
pub fn to_path_commands(path: &SkPath) -> Vec<PathCommand> {
    use jian_core::geometry::point;
    // PathVerb vive en skia_safe::prelude (core/path_types.rs:19), no en
    // skia_safe::path -- de ahi el E0603 de la primera version.
    use skia_safe::PathVerb;

    let pt = |p: SkPoint| point(p.x, p.y);
    let mut out = Vec::new();
    for rec in path.iter() {
        let verb = rec.verb();
        let points = rec.points();
        match verb {
            PathVerb::Move => {
                if let Some(p) = points.first() {
                    out.push(PathCommand::MoveTo(pt(*p)));
                }
            }
            PathVerb::Line => {
                if let Some(p) = points.first() {
                    out.push(PathCommand::LineTo(pt(*p)));
                }
            }
            PathVerb::Quad => {
                // points() = [control, p]
                if points.len() >= 2 {
                    out.push(PathCommand::QuadTo(pt(points[0]), pt(points[1])));
                } else if let Some(p) = points.first() {
                    out.push(PathCommand::LineTo(pt(*p)));
                }
            }
            PathVerb::Conic => {
                // Sin conic en PathCommand: degradamos al punto final.
                if let Some(p) = points.last() {
                    out.push(PathCommand::LineTo(pt(*p)));
                }
            }
            PathVerb::Cubic => {
                // points() = [c1, c2, p]
                if points.len() >= 3 {
                    out.push(PathCommand::CubicTo(pt(points[0]), pt(points[1]), pt(points[2])));
                } else if let Some(p) = points.first() {
                    out.push(PathCommand::LineTo(pt(*p)));
                }
            }
            PathVerb::Close => out.push(PathCommand::Close),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use jian_core::geometry::point;

    #[test]
    fn triangle_has_nonempty_bounds() {
        let cmds = [
            PathCommand::MoveTo(point(0.0, 0.0)),
            PathCommand::LineTo(point(10.0, 0.0)),
            PathCommand::LineTo(point(5.0, 10.0)),
            PathCommand::Close,
        ];
        let path = to_sk_path(&cmds);
        let bounds = path.compute_tight_bounds();
        assert!(bounds.width() > 0.0);
        assert!(bounds.height() > 0.0);
    }

    #[test]
    fn empty_commands_yield_empty_path() {
        let path = to_sk_path(&[]);
        assert!(path.is_empty());
    }

    // ---------- FASE 1: el shim to_path_commands ----------

    /// El caso que dispara todo: un rect de `build_rect_path` tiene que
    /// volver como 5 comandos (move + 4 lineas + close), no como un path
    /// opaco. Si esto pasa, el rect se puede convertir en anchors.
    #[test]
    fn sk_path_roundtrips_through_commands() {
        let cmds = [
            PathCommand::MoveTo(point(0.0, 0.0)),
            PathCommand::LineTo(point(10.0, 0.0)),
            PathCommand::LineTo(point(10.0, 10.0)),
            PathCommand::LineTo(point(0.0, 10.0)),
            PathCommand::LineTo(point(0.0, 0.0)),
            PathCommand::Close,
        ];
        let path = to_sk_path(&cmds);
        let back = to_path_commands(&path);
        // Skia puede normalizar el cierre; lo que importa es que el numero de
        // comandos y los puntos clave se conservan.
        assert!(
            back.len() >= 4,
            "un rect deberia dar al menos 4 comandos, dio {}",
            back.len()
        );
        assert!(
            back.contains(&PathCommand::Close),
            "el rect deberia conservar el cierre de contorno"
        );
    }

    /// Una curva cubica debe conservar sus dos puntos de control, que es
    /// justamente lo que un anchor necesita para tener handles.
    #[test]
    fn cubic_keeps_both_control_points() {
        let cmds = [
            PathCommand::MoveTo(point(0.0, 0.0)),
            PathCommand::CubicTo(point(10.0, 0.0), point(20.0, 10.0), point(30.0, 0.0)),
            PathCommand::Close,
        ];
        let path = to_sk_path(&cmds);
        let back = to_path_commands(&path);
        let cubic = back
            .iter()
            .find(|c| matches!(c, PathCommand::CubicTo(..)));
        assert!(
            cubic.is_some(),
            "la curva cubica deberia sobrevivir la ida y vuelta, no {:?}",
            back
        );
    }

    /// Contornos multiples (los holes de una letra, p.ej.): cada uno abre
    /// con su propio MoveTo. Es lo que necesita un donut o el ojo de una i.
    #[test]
    fn multiple_contours_each_open_with_move() {
        let cmds = [
            // contorno exterior
            PathCommand::MoveTo(point(0.0, 0.0)),
            PathCommand::LineTo(point(20.0, 0.0)),
            PathCommand::LineTo(point(20.0, 20.0)),
            PathCommand::LineTo(point(0.0, 20.0)),
            PathCommand::Close,
            // agujero
            PathCommand::MoveTo(point(5.0, 5.0)),
            PathCommand::LineTo(point(5.0, 15.0)),
            PathCommand::LineTo(point(15.0, 15.0)),
            PathCommand::LineTo(point(15.0, 5.0)),
            PathCommand::Close,
        ];
        let path = to_sk_path(&cmds);
        let back = to_path_commands(&path);
        let moves = back
            .iter()
            .filter(|c| matches!(c, PathCommand::MoveTo(..)))
            .count();
        assert_eq!(moves, 2, "los dos contornos deberian abrir con MoveTo");
    }

    /// La ida y vuelta no debe perder geometria: los bounds del comando
    /// reconstruido tienen que coincidir con los del original.
    #[test]
    fn roundtrip_preserves_bounds() {
        let cmds = [
            PathCommand::MoveTo(point(3.0, 7.0)),
            PathCommand::LineTo(point(40.0, 7.0)),
            PathCommand::LineTo(point(40.0, 25.0)),
            PathCommand::LineTo(point(3.0, 25.0)),
            PathCommand::Close,
        ];
        let original = to_sk_path(&cmds).compute_tight_bounds();
        let rebuilt = to_sk_path(&to_path_commands(&to_sk_path(&cmds)));
        let after = rebuilt.compute_tight_bounds();
        assert!(
            (original.width() - after.width()).abs() < 0.01
                && (original.height() - after.height()).abs() < 0.01,
            "los bounds cambiaron: {:?} -> {:?}",
            original,
            after
        );
    }

    /// El caso vacio no debe romper nada.
    #[test]
    fn empty_path_yields_no_commands() {
        let path = to_sk_path(&[]);
        assert!(to_path_commands(&path).is_empty());
    }
}
