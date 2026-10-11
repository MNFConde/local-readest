//! The online-library side: content-addressed catalog, OPDS feeds, and the
//! admin page. One switchable module tree (`library` cargo feature) so the
//! sync core ships without it and a future split into its own binary stays a
//! mechanical move. Everything here treats sync data (users/books/blobs) as
//! read-only except the catalog tables it owns.

pub mod catalog;
