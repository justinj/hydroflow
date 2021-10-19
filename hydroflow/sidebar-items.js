initSidebarItems({"enum":[["NullHandoff","A null handoff which will panic when called."]],"struct":[["Hydroflow","A Hydroflow graph. Owns, schedules, and runs the compiled subgraphs."],["InputPort","Handle corresponding to a [RecvCtx]. Consumed by [Hydroflow::add_edge] to construct the Hydroflow graph."],["OutputPort","Handle corresponding to a [SendCtx]. Consumed by [Hydroflow::add_edge] to construct the Hydroflow graph."],["RecvCtx","Context provided to a compiled component for reading from an [InputPort]."],["SendCtx","Context provided to a compiled component for writing to an [OutputPort]."],["VecHandoff","A [VecDeque]-based FIFO handoff."]],"trait":[["Handoff","A trait specifying a handoff point between compiled subgraphs."],["HandoffMeta","The metadata piece of a handoff."],["Readable","The read piece of a handoff."],["Writable","The write piece of a handoff."]]});