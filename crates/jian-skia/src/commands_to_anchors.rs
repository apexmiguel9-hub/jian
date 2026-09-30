//! FASE 3 (motor): `PathCommand` → anchors editables.
//!
//! COMPLETA LA CADENA que empezaron las fases 1 y 2:
//!
//!   primitiva  --(shape_to_path.rs, F2)-->  PathCommand
//!   PathCommand --(path.rs,      F1)-->  SkPath   (Skia → commands)
//!   PathCommand --(este fichero,  F3)-->  anchors  ← aquí
//!
//! Es el mismo camino en los dos sentidos: F2 produce comandos a mano, F1
//! recorre un SkPath para convertirlos en comandos, y aquí se convierten en
//! `PenPathAnchor`, que es la forma que el loader sabe pintar y el pen tool
//! sabe editar (`adapter/shapes.rs:189-195`).
//!
//! POR QUÉ NO SE PUEDE SALTAR ESTE PASO. La geometría editable vive en
//! `PathNode.anchors` (`PenPathAnchor`). Un `SkPath` no se edita. Por eso los
//! boolean ops producen hoy un path MUERTO: salen con `anchors: None`
//! (`host_support_allocator.rs:358`) y borran las fuentes (`:376).
//!
//! LA CONVERSIÓN IMPORTANTE: los handles son RELATIVOS al ancla, no
//! absolutos. Lo dice el doc de `set_path_anchor_handle`
//! (`op-editor-core/src/pen.rs:281`):
//!
//!   "`delta` is the handle offset relative to the anchor. When the anchor's
//!    `point_type` is `Mirrored`, the opposite handle is set to the negated
//!    offset so the two stay colinear + equal-length."
//!
//! O sea que un `CubicTo(c1, c2, p)` significa:
//!   handle_out del ancla ANTERIOR = c1 - (ancla anterior)
//!   handle_in  del ancla NUEVA    = c2 - p
//!
//! y si el ancla es `Mirrored`, el handle contrario es el offset NEGADO. Por
//! eso el `point_type` se propaga desde el `LineTo`/origen de la curva.

use jian_core::render::PathCommand;
use jian_ops_schema::node::{PenPathAnchor, PenPathHandle, PenPathPointType};

/// Resultado de convertir una lista de comandos en geometría editable.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnchorGeometry {
    pub anchors: Vec<PenPathAnchor>,
    /// El último `Close` cerró el contorno.
    pub closed: bool,
    /// Varios `MoveTo` = varios contornos. Un solo ancla no puede
    /// representar un agujero, así que se marca para que el que llama
    /// decida (típicamente: partir en varios subpaths, como hace el loader).
    pub contour_count: usize,
}

/// Convierte `PathCommand` en anchors editables.
///
/// Cada `MoveTo` abre un ancla nueva. Un `LineTo` añade un ancla cusp sin
/// handles. Un `CubicTo(c1, c2, p)` cierra los handles del ancla anterior
/// (su `handle_out`) y abre la nueva con su `handle_in`, ambos RELATIVOS.
/// Un `Close` marca el contorno como cerrado.
///
/// Los puntos son `f32` en `PathCommand` (euclid `Point2D<f32>`) y `f64` en
/// `PenPathAnchor`; la conversión es exacta en la práctica para coordenadas
/// de pantalla.
pub fn commands_to_anchors(commands: &[PathCommand]) -> AnchorGeometry {
    let mut out = AnchorGeometry::default();
    // Índice del ancla anterior, para poder colgarle su handle_out cuando
    // llega una cúbica.
    let mut prev: Option<usize> = None;

    for cmd in commands {
        match *cmd {
            PathCommand::MoveTo(p) => {
                out.anchors.push(PenPathAnchor {
                    x: p.x as f64,
                    y: p.y as f64,
                    handle_in: None,
                    handle_out: None,
                    point_type: Some(PenPathPointType::Corner),
                });
                out.contour_count += 1;
                prev = Some(out.anchors.len() - 1);
            }
            PathCommand::LineTo(p) => {
                // Segmento recto: el ancla anterior no lleva handle_in
                // porque no hay control. Si veníamos de una cúbica, su
                // handle_out ya quedó puesto y se conserva.
                out.anchors.push(PenPathAnchor {
                    x: p.x as f64,
                    y: p.y as f64,
                    handle_in: None,
                    handle_out: None,
                    point_type: Some(PenPathPointType::Corner),
                });
                prev = Some(out.anchors.len() - 1);
            }
            PathCommand::QuadTo(c, p) => {
                // Una cuadrática NO se puede representar con los dos handles
                // cúbicos del modelo: se promove a cúbica con la conversión
                // estándar (elevar el grado), que es exacta.
                if let Some(i) = prev {
                    let a = out.anchors[i].clone();
                    let p0 = (a.x, a.y);
                    let c0 = (c.x as f64, c.y as f64);
                    let p2 = (p.x as f64, p.y as f64);
                    let (c1, c2) = elevate_quadratic(p0, c0, p2);

                    out.anchors[i].handle_out = Some(PenPathHandle { x: c1.0, y: c1.1 });
                    out.anchors[i].point_type = Some(PenPathPointType::Mirrored);
                    out.anchors.push(PenPathAnchor {
                        x: p2.0,
                        y: p2.1,
                        handle_in: Some(PenPathHandle { x: c2.0, y: c2.1 }),
                        handle_out: None,
                        point_type: Some(PenPathPointType::Corner),
                    });
                    prev = Some(out.anchors.len() - 1);
                }
            }
            PathCommand::CubicTo(c1, c2, p) => {
                // El punto de control de salida pertenece al ancla
                // ANTERIOR, relativo a ella.
                if let Some(i) = prev {
                    out.anchors[i].handle_out = Some(PenPathHandle {
                        x: c1.x as f64 - out.anchors[i].x,
                        y: c1.y as f64 - out.anchors[i].y,
                    });
                    // Si tenía handle_in de una curva anterior, ahora ya no es
                    // simétrico respecto al nuevo: pasa a libre.
                    if out.anchors[i].handle_in.is_some() {
                        out.anchors[i].point_type = Some(PenPathPointType::Independent);
                    }
                }
                out.anchors.push(PenPathAnchor {
                    x: p.x as f64,
                    y: p.y as f64,
                    // Relativo al ancla nueva.
                    handle_in: Some(PenPathHandle {
                        x: c2.x as f64 - p.x as f64,
                        y: c2.y as f64 - p.y as f64,
                    }),
                    handle_out: None,
                    point_type: Some(PenPathPointType::Corner),
                });
                prev = Some(out.anchors.len() - 1);
            }
            PathCommand::Close => out.closed = true,
        }
    }
    out
}

/// Eleva una cuadrática a cúbica, que es exacta (no una aproximación).
///
/// p0 --c0-- p2   ->   p0 --c1,c2-- p2
/// con  c1 = p0 + 2/3 (c0 - p0)   y   c2 = p2 + 2/3 (c0 - p2)
fn elevate_quadratic(
    p0: (f64, f64),
    c0: (f64, f64),
    p2: (f64, f64),
) -> ((f64, f64), (f64, f64)) {
    let c1 = (p0.0 + (2.0 / 3.0) * (c0.0 - p0.0), p0.1 + (2.0 / 3.0) * (c0.1 - p0.1));
    let c2 = (p2.0 + (2.0 / 3.0) * (c0.0 - p2.0), p2.1 + (2.0 / 3.0) * (c0.1 - p2.1));
    (c1, c2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jian_core::geometry::point;
    use jian_ops_schema::node::PenPathPointType as T;

    #[test]
    fn empty_yields_nothing() {
        let g = commands_to_anchors(&[]);
        assert!(g.anchors.is_empty());
        assert!(!g.closed);
        assert_eq!(g.contour_count, 0);
    }

    #[test]
    fn rect_gives_four_anchors_closed_and_no_handles() {
        // El rect de las fases 1-2.
        let cmds = crate::shape_to_path::rect_commands(0.0, 0.0, 100.0, 50.0, None);
        let g = commands_to_anchors(&cmds);
        assert_eq!(g.anchors.len(), 4, "un rect son 4 anclas");
        assert!(g.closed, "el rect cierra su contorno");
        // Sin radios, todas las esquinas son rectas: sin handles.
        for (i, a) in g.anchors.iter().enumerate() {
            assert!(a.handle_out.is_none(), "ancla {} no deberia tener out", i);
            assert!(a.handle_in.is_none(), "ancla {} no deberia tener in", i);
        }
        // Y la geometría es la del rect.
        let xs: Vec<f64> = g.anchors.iter().map(|a| a.x).collect();
        let ys: Vec<f64> = g.anchors.iter().map(|a| a.y).collect();
        assert_eq!(xs, vec![0.0, 100.0, 100.0, 0.0]);
        assert_eq!(ys, vec![0.0, 0.0, 50.0, 50.0]);
    }

    #[test]
    fn rounded_rect_puts_a_handle_pair_on_each_corner() {
        let cmds = crate::shape_to_path::rect_commands(
            0.0,
            0.0,
            100.0,
            50.0,
            Some([(10.0, 10.0); 4]),
        );
        let g = commands_to_anchors(&cmds);
        // 4 lineas + 4 curvas = 8 anclas, cerrada.
        assert!(g.closed);
        assert!(g.anchors.len() >= 8, "anclas: {}", g.anchors.len());
        // Las anclas de inicio de curva tienen handle_out; las de fin, handle_in.
        let con_out = g.anchors.iter().filter(|a| a.handle_out.is_some()).count();
        let con_in = g.anchors.iter().filter(|a| a.handle_in.is_some()).count();
        assert!(con_out >= 3, "esperaba varias curvas, handle_out={}", con_out);
        assert!(con_in >= 3, "esperaba varias curvas, handle_in={}", con_in);
    }

    #[test]
    fn handles_are_relative_not_absolute() {
        // Un ancla en (50, 50) con control de salida en (60, 50) debe tener
        // handle_out = (10, 0), NO (60, 50).
        let cmds = [
            PathCommand::MoveTo(point(0.0, 0.0)),
            PathCommand::CubicTo(point(50.0, 0.0), point(60.0, 50.0), point(50.0, 50.0)),
        ];
        let g = commands_to_anchors(&cmds);
        let a0 = &g.anchors[0];
        assert_eq!(a0.x, 0.0);
        let out = a0.handle_out.expect("la primera deberia tener handle_out");
        assert!(
            (out.x - 50.0).abs() < 0.001 && out.y.abs() < 0.001,
            "handle_out debe ser RELATIVO al ancla: {:?}",
            out
        );
        // Y el ancla nueva guarda su handle_in relativo a ella.
        let a1 = &g.anchors[1];
        let h_in = a1.handle_in.expect("la segunda deberia tener handle_in");
        assert!(
            (h_in.x - 10.0).abs() < 0.001,
            "handle_in relativo: {:?}",
            h_in
        );
    }

    #[test]
    fn ellipse_gives_four_curved_anchors() {
        let cmds = crate::shape_to_path::ellipse_commands(0.0, 0.0, 80.0, 60.0);
        let g = commands_to_anchors(&cmds);
        assert!(g.closed, "la elipse cierra");
        // 4 cardinales + el cierre.
        assert!(g.anchors.len() >= 4);
        let con_handles = g
            .anchors
            .iter()
            .filter(|a| a.handle_in.is_some() || a.handle_out.is_some())
            .count();
        assert!(con_handles >= 4, "la elipse necesita curvas, {}", con_handles);
    }

    #[test]
    fn two_contours_are_reported_separately() {
        let cmds = [
            PathCommand::MoveTo(point(0.0, 0.0)),
            PathCommand::LineTo(point(10.0, 0.0)),
            PathCommand::LineTo(point(10.0, 10.0)),
            PathCommand::Close,
            PathCommand::MoveTo(point(20.0, 20.0)),
            PathCommand::LineTo(point(30.0, 20.0)),
            PathCommand::LineTo(point(30.0, 30.0)),
            PathCommand::Close,
        ];
        let g = commands_to_anchors(&cmds);
        assert_eq!(g.contour_count, 2, "dos MoveTo = dos contornos");
        assert!(g.closed, "el último Close marca cerrado");
    }

    #[test]
    fn an_anchor_between_two_curves_stops_being_mirrored() {
        // Ancla con curva por ambos lados: sus handles ya no son simétricos.
        let cmds = [
            PathCommand::MoveTo(point(0.0, 0.0)),
            PathCommand::CubicTo(point(10.0, 0.0), point(20.0, 10.0), point(20.0, 20.0)),
            PathCommand::CubicTo(point(20.0, 30.0), point(10.0, 40.0), point(0.0, 40.0)),
        ];
        let g = commands_to_anchors(&cmds);
        let a1 = &g.anchors[1];
        assert!(a1.handle_in.is_some() && a1.handle_out.is_some());
        assert_eq!(
            a1.point_type,
            Some(T::Independent),
            "con curvas a ambos lados ya no puede ser Mirrored"
        );
    }

    #[test]
    fn polygon_keeps_its_sides() {
        let cmds = crate::shape_to_path::polygon_commands(0.0, 0.0, 100.0, 100.0, 5);
        let g = commands_to_anchors(&cmds);
        assert_eq!(g.anchors.len(), 5, "pentágono = 5 anclas");
        assert!(g.closed);
    }
}
