//! The upgrade hand-off through rocket's `IoHandler`: rocket takes the hyper upgrade itself when the
//! app's 101 names `websocket`, writes the two upgrade headers, and hands the adapter an
//! `IoStream`, which becomes the app's `Upgraded`.
