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
use jian_ops_schema::node::PenNode;

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
    doc.insert_subtree(root, None);
    let mut engine = LayoutEngine::new();
    let root_id = engine.build(&doc).unwrap()[0];
    (doc, engine, root_id)
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

    for id in ["a", "b", "c"] {
        let rect = engine
            .node_rect(doc.get(id).unwrap())
            .unwrap_or_else(|| panic!("node_rect devolvio None para {id}"));
        println!(
            "PROBE|leaf {id}: size={}x{} origin=({},{})",
            rect.size.width, rect.size.height, rect.origin.x, rect.origin.y
        );
        assert_eq!(
            (rect.size.width, rect.size.height),
            (80.0_f32, 40.0_f32),
            "el rect {id} declaraba 80x40 pero taffy 0.14 le devolvio {}x{} -- el \
             delegate de measure devuelve Size::ZERO para hojas no-texto",
            rect.size.width, rect.size.height
        );
    }
}

/// TEST 2: los hijos se apilan en el flow con el gap correcto.
///
/// Es el presupuesto exacto que espera el host: con stack en y=60, alto 300,
/// gap 8 e hijos de 40, las posiciones dentro del stack son a=0, b=48, c=96
/// (40 de alto + 8 de gap). `flex_insert_preview` (drag_flow_index.rs:311)
/// compara el punto medio del arrastre contra los puntos medios de estos
/// rects, asi que si estas posiciones no son estas, el indice de insercion sale
/// mal.
#[test]
fn flow_children_stack_with_the_declared_gap() {
    let (doc, mut engine, root_id) = engine_for(VSTACK);
    engine.compute(root_id, (2000.0, 2000.0)).unwrap();

    let stack = engine.node_rect(doc.get("stack").unwrap()).unwrap();
    println!(
        "PROBE|stack: size={}x{} origin=({},{})",
        stack.size.width, stack.size.height, stack.origin.x, stack.origin.y
    );

    // (id, y esperado en coords absolutas)
    let expected = [("a", 60.0_f32), ("b", 108.0), ("c", 156.0)];

    for (id, expect_y) in expected {
        let rect = engine.node_rect(doc.get(id).unwrap()).unwrap();
        println!(
            "PROBE|child {id}: origin=({},{}) size={}x{}",
            rect.origin.x, rect.origin.y, rect.size.width, rect.size.height
        );
        assert_eq!(
            rect.origin.y, expect_y,
            "el hijo {id} deberia estar en y={expect_y} (stack en 60, 40 de alto + gap 8), \
             pero taffy lo puso en y={} con tamaño {}x{}",
            rect.origin.y, rect.size.width, rect.size.height
        );
    }
}

/// TEST 3: con size colapsado, el diagnostico del off-by-one.
///
/// Este test imprime los puntos medios que `flex_insert_preview` compararia,
/// para que quede constancia de por que el orden sale ["n100","a","b","c"].
/// Si los hijos colapsan a 0 de alto, todos los puntos medios caen en el
/// origen del padre y la comparacion `drag_mid < mid` del primer hermano
/// gana siempre, dando indice 0.
#[test]
fn diagnose_midpoints_used_by_flex_insert_preview() {
    let (doc, mut engine, root_id) = engine_for(VSTACK);
    engine.compute(root_id, (2000.0, 2000.0)).unwrap();

    // flex_insert_preview EXCLUYE al nodo arrastrado ('b'), asi que la lista
    // contra la que se compara es [a, c].
    let a = engine.node_rect(doc.get("a").unwrap()).unwrap();
    let c = engine.node_rect(doc.get("c").unwrap()).unwrap();
    let a_mid = a.origin.y + a.size.height / 2.0;
    let c_mid = c.origin.y + c.size.height / 2.0;

    // El test del host mueve 'b' 12px arriba: su mid pasa de 128 a 116.
    let drag_mid = 116.0_f32;

    println!("PROBE|mids: a={a_mid} (h={}), c={c_mid} (h={}), drag_mid={drag_mid}",
        a.size.height, c.size.height);

    let index = if drag_mid < a_mid {
        0
    } else if drag_mid < c_mid {
        1
    } else {
        2
    };
    assert_eq!(
        index, 1,
        "con el gap y los sizes correctos el indice de insercion debe ser 1 \
         (insertar antes que 'b'). Si sale 0, los rects que ve \
         drag_flow_index.rs estan colapsados. a_mid={a_mid} c_mid={c_mid} \
         drag_mid={drag_mid}"
    );
}
