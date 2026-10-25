use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    process::{Child, Command, Stdio},
    time::Duration,
};

struct Server {
    child: Child,
    address: SocketAddr,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Server {
    fn start() -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_conceptual-database"))
            .args(["--listen", "127.0.0.1:0"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut server = Self {
            child,
            address: "127.0.0.1:0".parse().unwrap(),
        };
        let stdout = server.child.stdout.take().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            BufReader::new(stdout).read_line(&mut line).unwrap();
            let _ = sender.send(line);
        });
        let line = receiver.recv_timeout(Duration::from_secs(10)).unwrap();
        server.address = line
            .trim()
            .strip_prefix("Database: http://")
            .unwrap()
            .parse()
            .unwrap();
        server
    }

    fn request(&self, method: &str, path: &str, body: Value) -> (u16, Value) {
        let body = if body.is_null() {
            String::new()
        } else {
            body.to_string()
        };
        let mut socket = TcpStream::connect_timeout(&self.address, Duration::from_secs(5)).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(socket, "{method} {path} HTTP/1.0\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", self.address, body.len()).unwrap();
        let mut response = String::new();
        socket.read_to_string(&mut response).unwrap();
        let (headers, body) = response.split_once("\r\n\r\n").unwrap();
        let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
        // Extractor rejections are text; successful API responses are JSON.
        let body = serde_json::from_str(body).unwrap_or_else(|_| Value::String(body.into()));
        (status, body)
    }
}

fn order(id: u64, customer: u64, state: &str, time: u64) -> Value {
    json!({"order_id": id, "customer_id": customer, "state": state, "product_id": 7, "placed_at": time})
}

#[test]
fn real_http_crud_queries_transactions_and_validation() {
    let server = Server::start();
    for row in [
        order(101, 42, "placed", 100),
        order(102, 42, "shipped", 200),
        order(103, 99, "placed", 300),
    ] {
        let id = row["order_id"].clone();
        assert_eq!(
            server.request("POST", "/orders", row),
            (201, json!({"order_id": id}))
        );
    }
    verify_queries(&server);
    verify_validation(&server);
    verify_writes_and_transactions(&server);
    verify_tree_swap(&server);
    let restarted = Server::start();
    assert_eq!(
        restarted.request("GET", "/orders", Value::Null).1["orders"],
        json!([])
    );
}

fn verify_queries(server: &Server) {
    let (status, primary) = server.request("GET", "/orders/101", Value::Null);
    assert_eq!(status, 200);
    assert_eq!(primary["orders"], json!([order(101, 42, "placed", 100)]));
    assert_eq!(primary["execution"]["plan"], "primary_lookup");
    let (status, state) = server.request("GET", "/orders?state=placed", Value::Null);
    assert_eq!(status, 200);
    assert_eq!(
        state["orders"],
        json!([order(101, 42, "placed", 100), order(103, 99, "placed", 300)])
    );
    assert_eq!(state["execution"]["records_examined"], 2);
    let (status, range) = server.request(
        "GET",
        "/orders?customer_id=42&from=100&through=200&latest=1",
        Value::Null,
    );
    assert_eq!(status, 200);
    assert_eq!(range["orders"], json!([order(102, 42, "shipped", 200)]));
    assert_eq!(range["execution"]["plan"], "customer_date_range");
    assert_eq!(range["execution"]["records_examined"], 1);
    let (status, scan) = server.request(
        "GET",
        "/orders?customer_id=42&from=100&through=200&latest=1&force_scan=true",
        Value::Null,
    );
    assert_eq!(status, 200);
    assert_eq!(scan["orders"], range["orders"]);
    assert_eq!(scan["execution"]["records_examined"], 3);
}

fn verify_validation(server: &Server) {
    assert_eq!(
        server
            .request("POST", "/orders", order(101, 1, "placed", 0))
            .0,
        409
    );
    assert_eq!(
        server
            .request("PUT", "/orders/101", order(999, 42, "placed", 0))
            .0,
        400
    );
    assert_eq!(
        server
            .request("GET", "/orders?from=200&through=100", Value::Null)
            .0,
        400
    );
    assert_eq!(
        server.request("GET", "/orders?latest=1", Value::Null).0,
        400
    );
    assert_eq!(
        server
            .request("GET", "/orders?customer_id=42&latest=0", Value::Null)
            .0,
        400
    );
    assert_eq!(
        server
            .request("GET", "/orders?state=unknown", Value::Null)
            .0,
        400
    );
    assert_eq!(
        server
            .request("GET", "/orders?customer_id=abc", Value::Null)
            .0,
        400
    );
    assert_eq!(server.request("GET", "/orders?typo=1", Value::Null).0, 400);
    assert_eq!(
        server.request("POST", "/orders", json!({"order_id": 1})).0,
        422
    );
    assert_eq!(
        server
            .request("PUT", "/orders/999", order(999, 42, "placed", 0))
            .0,
        404
    );
    assert_eq!(server.request("DELETE", "/orders/999", Value::Null).0, 404);
}

fn verify_writes_and_transactions(server: &Server) {
    assert_eq!(
        server
            .request("PUT", "/orders/101", order(101, 50, "shipped", 400))
            .0,
        204
    );
    let snapshot = server.request("GET", "/debug/indexes", Value::Null).1;
    assert_eq!(snapshot["by_state"]["placed"], json!([103]));
    assert_eq!(snapshot["by_state"]["shipped"], json!([101, 102]));
    assert_eq!(snapshot["by_customer_date"][0]["order_ids"], json!([102]));
    let batch = json!({"operations": [
        {"op": "create", "order": order(104, 42, "placed", 500)},
        {"op": "delete", "order_id": 102},
        {"op": "create", "order": order(103, 42, "placed", 500)}
    ]});
    assert_eq!(server.request("POST", "/transactions", batch).0, 409);
    assert_eq!(
        server.request("GET", "/debug/indexes", Value::Null).1,
        snapshot
    );
    let batch = json!({"operations": [
        {"op": "create", "order": order(104, 42, "placed", 500)},
        {"op": "replace", "order": order(102, 99, "cancelled", 600)},
        {"op": "delete", "order_id": 103}
    ]});
    assert_eq!(
        server.request("POST", "/transactions", batch),
        (200, json!({"applied": 3}))
    );
    assert_eq!(
        server.request("GET", "/orders/104", Value::Null).1["orders"],
        json!([order(104, 42, "placed", 500)])
    );
    assert_eq!(
        server.request("GET", "/orders/102", Value::Null).1["orders"],
        json!([order(102, 99, "cancelled", 600)])
    );
    assert_eq!(server.request("GET", "/orders/103", Value::Null).0, 404);
    assert_eq!(server.request("DELETE", "/orders/101", Value::Null).0, 204);
    assert_eq!(server.request("GET", "/orders/101", Value::Null).0, 404);
    assert_eq!(
        server.request("GET", "/debug/check", Value::Null),
        (200, json!({"consistent": true}))
    );
}

fn verify_tree_swap(server: &Server) {
    assert_eq!(
        server.request("GET", "/debug/tree", Value::Null),
        (200, json!({"tree": "std"}))
    );
    let query = "/orders?customer_id=42&latest=1";
    let before = server.request("GET", query, Value::Null).1;
    let snapshot = server.request("GET", "/debug/indexes", Value::Null).1;
    assert_eq!(
        server.request("PUT", "/debug/tree", json!({"tree": "bbtree"})),
        (200, json!({"tree": "bbtree"}))
    );
    let after = server.request("GET", query, Value::Null).1;
    assert_eq!(after["orders"], before["orders"]);
    assert_eq!(after["execution"]["tree"], "bbtree");
    let mut swapped = server.request("GET", "/debug/indexes", Value::Null).1;
    assert_eq!(swapped["tree"], "bbtree");
    swapped["tree"] = json!("std");
    assert_eq!(swapped, snapshot);
    assert_eq!(
        server.request("GET", "/debug/check", Value::Null),
        (200, json!({"consistent": true}))
    );
    assert_eq!(
        server
            .request("PUT", "/debug/tree", json!({"tree": "avl"}))
            .0,
        422
    );
}
