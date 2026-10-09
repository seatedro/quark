// Synthetic: comments in empty lists, before closers, and blank-line runs.
fn excerpt() {
    view! {
        <div a={1}


             b={2} /* inline */ c={3}>
            <div>
                // only a comment
            </div>
            <a/>



            <b/>
            // trailing note
        </div> // after root
    }
}
