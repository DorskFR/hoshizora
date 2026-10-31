use crate::proto::{FrameDown, Command};

pub async fn launch(bus: &Bus, id: u32) -> Result<(), Error> {
    let frame = FrameDown::Command { id, command: Box::new(Command::Spawn { id }) };
    bus.command_daemon(id, frame).await?;
    bus.notify(FrameDown::Reconcile { adapters: vec![] }).await;
    plain(frame);
    Ok(())
}

fn handle(frame: FrameDown) {
    match frame {
        FrameDown::Command { id, command } => run(id, command),
        FrameDown::Reconcile { adapters } => reconcile(adapters),
        _ => {}
    }
    if let Some(x) = maybe() {
        drop(x);
    }
    if let FrameDown::Ack(seq) = other() {
        ack(seq);
    }
}
