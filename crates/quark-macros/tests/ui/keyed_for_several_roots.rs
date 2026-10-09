use quark_macros::view;

fn main() {
    let items = [1, 2];
    let _ = view! {
        <div>
            for n in items key={n} {
                match n {
                    1 => <div />
                    _ => {
                        <div />
                        <div />
                    }
                }
            }
        </div>
    };
}
