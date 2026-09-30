//! FASE 2: primitivas → geometría editable, traduzco de Penpot.
//!
//! ORIGEN. Traducido de `render-wasm/src/shapes/shape_to_path.rs` de
//! `penpot/penpot` (282 líneas, MPL-2.0), más `BEZIER_CIRCLE_C` y el clamp
//! de radios del W3C que Penpot implementa ahí.
//!
//! POR QUÉ ESTE FICHERO EXISTE. Con `to_path_commands` (path.rs, Fase 1)
//! ya sabemos Skia → PathCommand. Falta el otro sentido a mano: sacar
//! PathCommand de una PRIMITIVA (rect, elipse, polígono, línea) de forma que
//! el loader lo convierta en `anchors` de verdad. Ese es el "convertir objeto
//! a trazos" del menú de Diseño.
//!
//! POR QUÉ NO ES `build_rect_path` (boolean_ops.rs:110). Eso ya existe, pero
//! produce un `SkPath` opaco: sirve para boolean ops, no para editar
//! vértices. Lo que sale de aquí son comandos, y de ahí el loader traza los
//! anchors. Mismo rect, distinto destino.
//!
//! DIFERENCIA CON `build_rect_path`: aquí el corner-radius se respeta POR
//! ESQUINA, con el clamp del CSS. `build_rect_path` llama a
//! `PathBuilder::add_rect` con un radio uniforme, así que un rect con
//! esquinas distintas se renderiza bien pero no se puede editar con
//! fidelidad al reconstruirlo como path.
//!
//! `fix_radius` implementa la spec W3C que Penpot cita textualmente
//! (CSS Backgrounds 3 §"Corner curves must not overlap"): si la suma de dos
//! radios adyacentes excede el lado, se escalan proporcionalmente.

use crate::render::PathCommand;

/// La constante mágica de Bézier para aproximar un círculo: 4·(√2−1)/3.
/// Igual que en Penpot (`shape_to_path.rs:7`).
const BEZIER_CIRCLE_C: f32 = 0.551_915_05;

/// Mínimo de cinco f32 (como el `min_5` de Penpot).
fn min_5(a: f32, b: f32, c: f32, d: f32, e: f32) -> f32 {
    f32::min(a, f32::min(b, f32::min(c, f32::min(d, e))))
}

/// Los cuatro radios, en el orden TL, TR, BR, BL.
type Radii = [(f32, f32); 4];

/// Escala los radios si se solapan, según la spec W3C.
///
/// Translated de `fix_radius` (Penpot, shape_to_path.rs:81), que cita la spec:
///
/// > Let f = min(Li/Si) ... If f < 1, then all corner radii are reduced
/// > by multiplying them by f.
///
/// con L = ancho (top/bottom) o alto (left/right), y S = suma de los dos
/// radios de las esquinas de ese lado.
pub fn fix_radius(r: Radii, width: f32, height: f32) -> Radii {
    let safe = |d: f32| if d > 0.0 { d } else { f32::INFINITY };
    let f = min_5(
        1.0,
        width / safe(r[0].0 + r[1].0),
        height / safe(r[1].1 + r[2].1),
        width / safe(r[2].0 + r[3].0),
        height / safe(r[3].1 + r[0].1),
    );
    if f < 1.0 {
        [
            (r[0].0 * f, r[0].1 * f),
            (r[1].0 * f, r[1].1 * f),
            (r[2].0 * f, r[2].1 * f),
            (r[3].0 * f, r[3].1 * f),
        ]
    } else {
        r
    }
}

/// Un rectángulo alineado a ejes como PathCommand, con radio por esquina.
///
/// Traducido de `rect_segments_local` (Penpot, shape_to_path.rs:113) y su
/// helper `make_corner`. Devuelve siempre un contorno cerrado.
pub fn rect_commands(x: f32, y: f32, width: f32, height: f32, radii: Option<Radii>) -> Vec<PathCommand> {
    let Some([r1, r2, r3, r4]) = radii else {
        // Sin radios: un rect simple. Mismo resultado que add_rect pero
        // como comandos, para que el loader pueda trazar anchors.
        return vec![
            PathCommand::MoveTo(jpoint(x, y)),
            PathCommand::LineTo(jpoint(x + width, y)),
            PathCommand::LineTo(jpoint(x + width, y + height)),
            PathCommand::LineTo(jpoint(x, y + height)),
            PathCommand::Close,
        ];
    };

    let [r1, r2, r3, r4] = fix_radius([r1, r2, r3, r4], width, height);
    // Si tras el clamp todo es cero, es un rect recto: no gastes curva.
    if r1 == (0.0, 0.0) && r2 == (0.0, 0.0) && r3 == (0.0, 0.0) && r4 == (0.0, 0.0) {
        return rect_commands(x, y, width, height, None);
    }

    // Los 8 puntos de borde, en sentido horario desde el inicio del borde
    // superior (traducido literal de Penpot, :119-126).
    let p1 = (x, y + r1.1);
    let p2 = (x + r1.0, y);
    let p3 = (x + width - r2.0, y);
    let p4 = (x + width, y + r2.1);
    let p5 = (x + width, y + height - r3.1);
    let p6 = (x + width - r3.0, y + height);
    let p7 = (x + r4.0, y + height);
    let p8 = (x, y + height - r4.1);

    vec![
        PathCommand::MoveTo(jpoint(p1.0, p1.1)),
        corner(Corner::TopLeft, p1, p2, r1),
        PathCommand::LineTo(jpoint(p3.0, p3.1)),
        corner(Corner::TopRight, p3, p4, r2),
        PathCommand::LineTo(jpoint(p5.0, p5.1)),
        corner(Corner::BottomRight, p5, p6, r3),
        PathCommand::LineTo(jpoint(p7.0, p7.1)),
        corner(Corner::BottomLeft, p7, p8, r4),
        PathCommand::Close,
    ]
}

/// Una elipse (o círculo) como PathCommand, cerrada.
///
/// Traducido de `circle_segments` (Penpot, :167). Cuatro cúbicas con
/// `BEZIER_CIRCLE_C`, que es la aproximación estándar del círculo.
pub fn ellipse_commands(x: f32, y: f32, width: f32, height: f32) -> Vec<PathCommand> {
    let c = BEZIER_CIRCLE_C;
    let hw = width / 2.0;
    let hh = height / 2.0;
    let cx = x + hw;
    let cy = y + hh;

    // Puntos cardinales.
    let left = (x, cy);
    let top = (cx, y);
    let right = (x + width, cy);
    let bottom = (cx, y + height);

    // Controles: cada cuadrante se dibuja con dos puntos de control a
    // distancia c * radio desde el centro de la elipse.
    let dx = hw * c;
    let dy = hh * c;

    vec![
        PathCommand::MoveTo(jpoint(left.0, left.1)),
        // izquierda -> arriba
        PathCommand::CubicTo(
            jpoint(left.0, left.1 - dy),
            jpoint(top.0 - dx, top.1),
            jpoint(top.0, top.1),
        ),
        // arriba -> derecha
        PathCommand::CubicTo(
            jpoint(top.0 + dx, top.1),
            jpoint(right.0, right.1 - dy),
            jpoint(right.0, right.1),
        ),
        // derecha -> abajo
        PathCommand::CubicTo(
            jpoint(right.0, right.1 + dy),
            jpoint(bottom.0 + dx, bottom.1),
            jpoint(bottom.0, bottom.1),
        ),
        // abajo -> izquierda
        PathCommand::CubicTo(
            jpoint(bottom.0 - dx, bottom.1),
            jpoint(left.0, left.1 + dy),
            jpoint(left.0, left.1),
        ),
        PathCommand::Close,
    ]
}

/// Un polígono regular de `sides` lados, circunscrito en el rect dado.
pub fn polygon_commands(x: f32, y: f32, width: f32, height: f32, sides: u32) -> Vec<PathCommand> {
    let sides = sides.max(3);
    let cx = x + width / 2.0;
    let cy = y + height / 2.0;
    let rx = width / 2.0;
    let ry = height / 2.0;

    let mut out = Vec::with_capacity(sides as usize + 1);
    for i in 0..sides {
        // Empezar en -90 grados para que el primer vértice quede arriba, que
        // es lo que espera cualquiera que dibuja un polígono.
        let angle = -std::f32::consts::FRAC_PI_2
            + (i as f32) * 2.0 * std::f32::consts::PI / (sides as f32);
        let px = cx + rx * angle.cos();
        let py = cy + ry * angle.sin();
        if i == 0 {
            out.push(PathCommand::MoveTo(jpoint(px, py)));
        } else {
            out.push(PathCommand::LineTo(jpoint(px, py)));
        }
    }
    out.push(PathCommand::Close);
    out
}

/// Una línea simple (2 puntos), sin cerrar.
pub fn line_commands(x1: f32, y1: f32, x2: f32, y2: f32) -> Vec<PathCommand> {
    vec![
        PathCommand::MoveTo(jpoint(x1, y1)),
        PathCommand::LineTo(jpoint(x2, y2)),
    ]
}

/// Las cuatro esquinas de un rect, para `make_corner`.
#[derive(Clone, Copy)]
enum Corner {
    TopLeft,
    TopRight,
    BottomRight,
    BottomLeft,
}

/// La cúbica de una esquina redondeada, de `from` a `to`.
///
/// Traducido de `make_corner` (Penpot, :14-64). La esquina son DOS puntos
/// de control en una sola cúbica, que es como un round-join de un solo
/// segmento.
fn corner(kind: Corner, from: (f32, f32), to: (f32, f32), r: (f32, f32)) -> PathCommand {
    let c = BEZIER_CIRCLE_C;
    let width = r.0 * 2.0;
    let height = r.1 * 2.0;

    let (x, y) = match kind {
        Corner::TopRight => (from.0 - r.0, from.1),
        Corner::TopLeft => from,
        Corner::BottomRight => (from.0, to.1 - height),
        Corner::BottomLeft => to,
    };

    let c1x = x + (width / 2.0) * (1.0 - c);
    let c2x = x + (width / 2.0) * (1.0 + c);
    let c1y = y + (height / 2.0) * (1.0 - c);
    let c2y = y + (height / 2.0) * (1.0 + c);

    let h1 = match kind {
        Corner::TopLeft => (from.0, c1y),
        Corner::TopRight => (c2x, from.1),
        Corner::BottomRight => (from.0, c2y),
        Corner::BottomLeft => (c1x, from.1),
    };
    let h2 = match kind {
        Corner::TopLeft => (c1x, to.1),
        Corner::TopRight => (to.0, c1y),
        Corner::BottomRight => (c2x, to.1),
        Corner::BottomLeft => (to.0, c2y),
    };

    PathCommand::CubicTo(jpoint(h1.0, h1.1), jpoint(h2.0, h2.1), jpoint(to.0, to.1))
}

fn jpoint(x: f32, y: f32) -> crate::geometry::Point {
    crate::geometry::point(x, y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::point;

    /// Caja de los PUNTOS DE CONTROL de los comandos, sin skia. Para las
    /// formas de este modulo (rect, elipse, poligono) los controles nunca se
    /// salen de la caja pedida, asi que la caja de los puntos es la caja real.
    fn bounds_of(cmds: &[PathCommand]) -> (f32, f32) {
        let mut minx = f32::INFINITY;
        let mut miny = f32::INFINITY;
        let mut maxx = f32::NEG_INFINITY;
        let mut maxy = f32::NEG_INFINITY;
        let mut see = |p: crate::geometry::Point| {
            minx = minx.min(p.x);
            miny = miny.min(p.y);
            maxx = maxx.max(p.x);
            maxy = maxy.max(p.y);
        };
        for c in cmds {
            match *c {
                PathCommand::MoveTo(p) | PathCommand::LineTo(p) => see(p),
                PathCommand::QuadTo(c1, p) => {
                    see(c1);
                    see(p);
                }
                PathCommand::CubicTo(c1, c2, p) => {
                    see(c1);
                    see(c2);
                    see(p);
                }
                PathCommand::Close => {}
            }
        }
        (maxx - minx, maxy - miny)
    }

    #[test]
    fn plain_rect_has_requested_size() {
        let cmds = rect_commands(10.0, 20.0, 100.0, 50.0, None);
        let (w, h) = bounds_of(&cmds);
        assert!((w - 100.0).abs() < 0.01, "ancho: {}", w);
        assert!((h - 50.0).abs() < 0.01, "alto: {}", h);
    }

    #[test]
    fn rounded_rect_keeps_size_and_is_closed() {
        let cmds = rect_commands(0.0, 0.0, 100.0, 50.0, Some([(8.0, 8.0); 4]));
        let (w, h) = bounds_of(&cmds);
        assert!((w - 100.0).abs() < 0.5, "ancho tras redondear: {}", w);
        assert!((h - 50.0).abs() < 0.5, "alto tras redondear: {}", h);
        assert!(
            cmds.iter().any(|c| matches!(c, PathCommand::Close)),
            "un rect redondeado debe cerrar su contorno"
        );
        // 4 esquinas = 4 cubicas, más 4 lineas + move + close.
        let cubics = cmds
            .iter()
            .filter(|c| matches!(c, PathCommand::CubicTo(..)))
            .count();
        assert_eq!(cubics, 4, "debería haber una cúbica por esquina");
    }

    #[test]
    fn per_corner_radii_are_respected() {
        // Solo la esquina superior izquierda redondeada.
        let cmds = rect_commands(0.0, 0.0, 100.0, 50.0, Some([(10.0, 10.0), (0.0, 0.0), (0.0, 0.0), (0.0, 0.0)]));
        let cubics = cmds
            .iter()
            .filter(|c| matches!(c, PathCommand::CubicTo(..)))
            .count();
        assert_eq!(cubics, 1, "solo una esquina redondeada -> una cúbica");
    }

    #[test]
    fn fix_radius_shrinks_overlapping_corners() {
        // Dos radios de 60 en un lado de 100 se solapan: deben escalar.
        let out = fix_radius([(60.0, 60.0); 4], 100.0, 100.0);
        assert!(out[0].0 < 60.0, "el radio debe reducirse: {}", out[0].0);
    }

    #[test]
    fn fix_radius_leaves_valid_corners_alone() {
        let out = fix_radius([(8.0, 8.0); 4], 100.0, 100.0);
        assert_eq!(out[0], (8.0, 8.0), "no hay por qué tocar un radio válido");
    }

    #[test]
    fn ellipse_matches_requested_box() {
        let cmds = ellipse_commands(0.0, 0.0, 80.0, 60.0);
        let (w, h) = bounds_of(&cmds);
        assert!((w - 80.0).abs() < 0.5, "ancho de la elipse: {}", w);
        assert!((h - 60.0).abs() < 0.5, "alto de la elipse: {}", h);
        let cubics = cmds
            .iter()
            .filter(|c| matches!(c, PathCommand::CubicTo(..)))
            .count();
        assert_eq!(cubics, 4, "una elipse son 4 cúbicas");
    }

    #[test]
    fn polygon_has_requested_side_count() {
        let cmds = polygon_commands(0.0, 0.0, 100.0, 100.0, 6);
        // 1 move + 5 lineas + close
        assert_eq!(cmds.len(), 7, "hexágono: 1 move + 5 lineas + close");
        let (w, h) = bounds_of(&cmds);
        assert!((w - 100.0).abs() < 0.5 && (h - 100.0).abs() < 0.5, "caja del polígono: {}x{}", w, h);
    }

    #[test]
    fn line_is_two_points_and_not_closed() {
        let cmds = line_commands(0.0, 0.0, 30.0, 40.0);
        assert_eq!(cmds.len(), 2);
        assert!(!cmds.iter().any(|c| matches!(c, PathCommand::Close)));
    }

}