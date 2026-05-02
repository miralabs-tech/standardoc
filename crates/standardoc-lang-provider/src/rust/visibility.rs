use standardoc_ir::Visibility;

pub(crate) fn map(vis: &syn::Visibility) -> Visibility {
    match vis {
        syn::Visibility::Public(_) => Visibility::Public,
        syn::Visibility::Inherited => Visibility::Private,
        syn::Visibility::Restricted(restricted) => {
            if restricted.path.is_ident("crate") {
                Visibility::Crate
            } else if restricted.path.is_ident("self") {
                Visibility::Private
            } else {
                Visibility::Crate
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> syn::Visibility {
        syn::parse_str(input).expect("test visibility input not parsable")
    }

    #[test]
    fn public_keyword_maps_to_public() {
        assert_eq!(map(&parse("pub")), Visibility::Public);
    }

    #[test]
    fn inherited_maps_to_private() {
        assert_eq!(map(&syn::Visibility::Inherited), Visibility::Private);
    }

    #[test]
    fn pub_crate_maps_to_crate() {
        assert_eq!(map(&parse("pub(crate)")), Visibility::Crate);
    }

    #[test]
    fn pub_super_maps_to_crate() {
        assert_eq!(map(&parse("pub(super)")), Visibility::Crate);
    }

    #[test]
    fn pub_self_maps_to_private() {
        assert_eq!(map(&parse("pub(self)")), Visibility::Private);
    }

    #[test]
    fn pub_in_path_maps_to_crate() {
        assert_eq!(map(&parse("pub(in crate::foo::bar)")), Visibility::Crate);
    }
}
