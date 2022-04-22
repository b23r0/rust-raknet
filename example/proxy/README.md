## A Simple Raknet Proxy

## Usage

```
Usage: proxy [-l] [LOCAL_ADDR] [-r] [REMOTE_ADDRESS]

Options:
    -l, --local_address LISTEN_ADDR
                        The address on which to listen raknet proxy for
                        incoming requests
    -r, --remote_address REMOTE_ADDRESS
                        The address is remote raknet server
```

## Example 

`.\proxy.exe -l 0.0.0.0:8000 -r 127.0.0.1:19132`