//! Reproduccion del colapso de sizing en hojas no-texto tras el upgrade a
//! Taffy 0.14.
//!
//! CONTEXTO: el upgrade a Taffy 0.14 (commit 0031567) cambio el contrato del
//! callback `measure`. En 0.5.x, Taffy post-procesaba el Size devuelto con
//! `known_dimensions.or(style.size).unwrap_or(measured + inset)` y clamps
//! min/max. En 0.14 se usa el `LayoutOutput` del callback VERBATIM, y el
//! delegate `compute_leaf_layout` solo tiene en cuenta el `Size` que devuelve
//! la funcion de measure interna.
//!
//! En mod.rs el callback devuelve `Size::ZERO` para todo lo que no sea texto
//! (linea ~367):
//!
//!     taffy::compute::compute_leaf_layout(inputs, style, |_, value| value, |known, avail| {
//!         if let Some(inner) = ctx {
//!             if let Some(tm) = inner.as_ref() {
//!                 return measure_text_for_taffy(backend.as_ref(), tm, known, avail);
//!             }
//!         }
//!         Size::ZERO
//!     })
//!
//! Ese `Size::ZERO` es correcto SOLO si `compute_leaf_layout` lo combina con
//! `style.size` / `known_dimensions`. Este test verifica si ocurre, usando el
//! fixture exacto de `canvas_select_drag_tests::VSTACK`, que es el que produce
//! el off-by-one de reordenacion (`["n100","a","b","c"]` en vez de
//! `["a","n100","b","c"]`).
//!
//! Si este test falla, el fix NO esta en el editor (drag_flow_index.rs) sino
//! en el delegate de measure de jian-core.

use super::*;
use crate::document::NodeTree;
use crate::error::{CoreError, CoreResult};
use jian_ops_schema::node::PenNode;
use slotmap::SecondaryMap;
use taffy::prelude::*;

/// El mismo fixture que `VSTACK` en canvas_select_drag_tests.rs: frame
/// vertical en (400,60) 200x300, gap 8, tres rectangulos hijos de 80x40.
const VSTACK: &str = r#"{"version":"1.0.0","children":[
  {"type":"frame","id":"stack","name":"Stack","x":400,"y":60,"width":200,"height":300,
   "layout":"vertical","gap":8,
   "children":[
     {"type":"rectangle","id":"a","name":"A","width":80,"height":40},
     {"type":"rectangle","id":"b","name":"B","width":80,"height":40},
     {"type":"rectangle","id":"c","name":"C","width":80,"height":40}
   ]}
]}"#;

fn engine_for(json: &str) -> (NodeTree, LayoutEngine, NodeId) {
    let root: PenNode = serde_json::from_str(json).unwrap();
    let mut doc = NodeTree::new();
    let root_key = doc.insert_subtree(root, None);
    let mut engine = LayoutEngine::new();
    let root_id = engine.build(&doc).unwrap()[0];
    (doc, engine, root_id)
}

/// Localiza la clave de un nodo por id dentro del NodeTree.
fn key_by_id(tree: &NodeTree, id: &str) -> Option<crate::document::NodeKey> {
    tree.nodes.iter().find_map(|(key, data)| {
        (data.schema.id_str() == id).then_some(key)
    })
}

/// TEST 1: los rectangulos (hojas no-texto) conservan su size 80x40.
///
/// Este es el corazon del asunto. Si un rect de 80x40 colapsa a 0x0, el
/// motor no tiene donde colocar a los hermanos, y eso se manifiesta como
/// los objetos que "se repelen" y no anclan.
#[test]
fn non_text_leaf_keeps_its_authored_size() {
    let (doc, mut engine, root_id) = engine_for(VSTACK);
    engine.compute(root_id, (2000.0, 2000.0)).unwrap();

    for (id, expect_w, expect_h) in [("a", 80.0_f32, 40.0_f32), ("b", 80.0, 40.0), ("c", 80.0, 40.0)] {
        let key = key_by_id(&doc, id).unwrap_or_else(|| panic!("no encuentro el nodo {id}"));
        let rect = engine
            .node_rect(&doc, key)
            .unwrap_or_else(|| panic!("node_rect devolvio None para {id}"));
        assert_eq!(
            (rect.width, rect.height),
            (expect_w, expect_h),
            "el rect {id} declaraba {expect_w}x{expect_h} pero taffy 0.14 le devolvio \
             {}x{} -- el delegate de measure devuelve Size::ZERO para hojas no-texto",
            rect.width, rect.height
        );
    }
}

/// TEST 2: los hijos se apilan en el flow con el gap correcto.
///
/// Es el presupuesto exacto que espera el host: con stack en y=60, alto 300,
/// gap 8 e hijos de 40, las posiciones absolutas son a=60, b=108, c=156.
/// `flex_insert_preview` (drag_flow_index.rs:311) compara el punto medio del
/// arrastre contra los puntos medios de estos rects, asi que si estas
/// posiciones no son estas, el indice de insercion sale mal.
#[test]
fn flow_children_stack_with_the_declared_gap() {
    let (doc, mut engine, root_id) = engine_for(VSTACK);
    engine.compute(root_id, (2000.0, 2000.0)).unwrap();

    // (id, y esperado en coords del stack, y medio esperado)
    let expected = [("a", 0.0_f32), ("b", 48.0), ("c", 96.0)];

    for (id, expect_y) in expected {
        let key = key_by_id(&doc, id).unwrap();
        let rect = engine.node_rect(&doc, key).unwrap();
        assert_eq!(
            rect.y, expect_y,
            "el hijo {id} deberia estar en y={expect_y} dentro del stack (40 de alto + gap 8), \
             pero taffy lo puso en y={} con tamaño {}x{}",
            rect.y, rect.width, rect.height
        );
    }
}

/// TEST 3: con size colapsado, el diagnostico del off-by-one.
///
/// Este test imprime los puntos medios que `flex_insert_preview` compararia,
/// para que quede constancia de por que el orden sale ["n100","a","b","c"].
/// Si los hijos colapsan a 0 de alto, todos los puntos medios coinciden con el
/// origen y la comparacion `drag_mid < mid` del primer hermano gana siempre,
/// dando indice 0.
#[test]
fn diagnose_midpoints_used_by_flex_insert_preview() {
    let (doc, mut engine, root_id) = engine_for(VSTACK);
    engine.compute(root_id, (2000.0, 2000.0)).unwrap();

    let mut mids = Vec::new();
    for (id, _expect_y) in [("a", 0.0_f32), ("b", 48.0), ("c", 96.0)] {
        let key = key_by_id(&doc, id).unwrap();
        let rect = engine.node_rect(&doc, key).unwrap();
        mids.push((id, rect.y, rect.height, rect.y + rect.height / 2.0));
    }

    println!("PROBE|leaf-sizing mids (id, y, height, mid):");
    for (id, y, h, mid) in &mids {
        println!("PROBE|   {id}: y={y} height={h} mid={mid}");
    }

    // El drag del test mueve 'b' 12px arriba: su mid pasa de 48+20=68 a 56.
    let drag_mid = 56.0_f32;
    // flex_insert_preview excluye al nodo arrastrado, asi que compara contra
    // los mids de 'a' (20) y 'c' (116) en el espacio ya scrolleado.
    let a_mid = 0.0_f32 + 40.0 / 2.0;
    let c_mid = 96.0_f32 + 40.0 / 2.0;
    let expected_index = if drag_mid < a_mid {
        0
    } else if drag_mid < c_mid {
        1
    } else {
        2
    };
    assert_eq!(
        expected_index, 1,
        "con el gap correcto el indice de insercion debe ser 1; si sale 0, los \
         rects que ve drag_flow_index.rs estan colapsados. mids observados: {mids:?}"
    );
}
