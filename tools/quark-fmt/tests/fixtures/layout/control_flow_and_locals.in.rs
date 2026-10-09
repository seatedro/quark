// Synthetic: locals keep their place and spelling; branches keep their shape.
fn excerpt() {
    view! {
        <div>
            for (i, item) in items.iter().enumerate() key={item.id} {
                let key = format!("item-{i}");
                let Some(label) = item.label.as_ref() else { continue };
                if let Some(x) = item.x && x > 0 { <a/> } else if i == 0 { <b/> } else {}
            }
            for x in xs { {x} }
        </div>
    }
}
