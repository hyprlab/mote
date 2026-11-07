//! Wake-on-LAN: the magic packet, six bytes of 0xFF and the MAC address
//! sixteen times, broadcast on the local network.

use std::net::Ipv4Addr;

pub(super) async fn send(mac: [u8; 6]) {
    let packet = packet(mac);
    let socket = match tokio::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await {
        Ok(socket) => socket,
        Err(e) => {
            tracing::warn!("Wake-on-LAN: {e}");
            return;
        }
    };
    if let Err(e) = socket.set_broadcast(true) {
        tracing::warn!("Wake-on-LAN: {e}");
        return;
    }
    // Port 9, discard, is the usual one; some network cards listen on 7.
    for port in [9, 7] {
        if let Err(e) = socket.send_to(&packet, (Ipv4Addr::BROADCAST, port)).await {
            tracing::warn!("Wake-on-LAN to port {port}: {e}");
        }
    }
    tracing::info!("sent a Wake-on-LAN packet");
}

fn packet(mac: [u8; 6]) -> Vec<u8> {
    let mut packet = vec![0xFF; 6];
    for _ in 0..16 {
        packet.extend_from_slice(&mac);
    }
    packet
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_packet_repeats_the_address() {
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let packet = super::packet(mac);
        assert_eq!(packet.len(), 102);
        assert_eq!(&packet[..6], &[0xFF; 6]);
        assert!(packet[6..].chunks(6).all(|chunk| chunk == mac));
    }
}
