//! Entry point. Kept to wiring only; logic lives in the library so it can be
//! tested.

fn main() {
    let roles: Vec<&str> = oss_node::enabled_roles()
        .into_iter()
        .map(oss_node::Role::name)
        .collect();
    println!("opensearch-serverless node, roles: {}", roles.join(", "));
}
