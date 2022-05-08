package io.wcygan;

import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpServer;

import java.io.IOException;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.util.Objects;
import java.util.Optional;
import java.util.concurrent.ForkJoinPool;

public class App {
    private static final Integer PORT = 8000;
    private static final Integer HTTP_OKAY = 200;

    public static void main(String[] args) throws IOException {
        HttpServer server = HttpServer.create(new InetSocketAddress(PORT), 0);
        server.setExecutor(ForkJoinPool.commonPool());

        server.createContext("/", handler -> {
            String query = handler.getRequestURI().getRawQuery();

            String response = parse(query)
                    .map(App::fib)
                    .map(Objects::toString)
                    .orElseGet(() -> query + " is not an Integer!");

            send(handler, response + "\n");
        });

        System.out.println("Server is listening for requests on port " + PORT);
        server.start();
    }

    // Calculate the n-th fibonacci number
    static int fib(int n) {
        if (n == 0 || n == 1) return n;
        return fib(n - 1) + fib(n - 2);
    }

    // Parse a query string
    static Optional<Integer> parse(String query) {
        try {
            return Optional.of(Integer.parseInt(query));
        } catch (NumberFormatException n) {
            return Optional.empty();
        }
    }

    // Send a response to a client
    static void send(HttpExchange handler, String response) throws IOException {
        OutputStream out = handler.getResponseBody();
        handler.sendResponseHeaders(HTTP_OKAY, response.length());
        out.write(response.getBytes());
        out.close();
    }
}
