package com.aieink.pokedashboard;

import org.json.JSONObject;

import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetAddress;
import java.nio.charset.StandardCharsets;

final class DiscoveryClient {
    private static final int DISCOVERY_PORT = 8766;
    private static final byte[] REQUEST = "AI_EINK_DISCOVER".getBytes(StandardCharsets.UTF_8);

    private DiscoveryClient() {}

    static String discover() {
        try (DatagramSocket socket = new DatagramSocket()) {
            socket.setBroadcast(true);
            socket.setSoTimeout(2500);
            DatagramPacket request = new DatagramPacket(
                    REQUEST, REQUEST.length, InetAddress.getByName("255.255.255.255"), DISCOVERY_PORT);
            socket.send(request);
            byte[] buffer = new byte[512];
            DatagramPacket response = new DatagramPacket(buffer, buffer.length);
            socket.receive(response);
            JSONObject payload = new JSONObject(new String(
                    response.getData(), 0, response.getLength(), StandardCharsets.UTF_8));
            int port = payload.optInt("port", 8765);
            return "http://" + response.getAddress().getHostAddress() + ":" + port;
        } catch (Exception ignored) {
            return null;
        }
    }
}
