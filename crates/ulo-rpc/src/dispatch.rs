//! Per-delivery execution (transports DESIGN §5.2): one execution per call, `deadline-ms` as its
//! deadline, `cancel` as `ClientCancelled`, the four shapes, `dispatch_late` for an item's error,
//! and the split per `Capabilities` between the whole frame and the payload with native
//! correlation. `Server::serve` runs it for every delivery the link's inbound stream yields.
