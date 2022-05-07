# Java Web Server

## Build & Run

The root directory of this project is `examples/java-web-server`

You can open the root directory in IntelliJ to edit & index the project.

You can run the project with this command: 

```zsh
$ gradle run
```

## Sending requests to the server

You can use curl to send requests to the server:

```zsh
$ curl localhost:8000?30
```

### Sending multiple requests in parallel

You can combine `seq`, `xargs`, and `curl` to send multiple requests in parallel:

```zsh
$ seq 1 10 | xargs -n1 -P10 curl "localhost:8000?30"
```