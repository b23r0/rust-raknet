// Independent RakNet v11 peer for transport interoperability checks.
package main

import (
	"bytes"
	"context"
	"fmt"
	"log/slog"
	"net"
	"os"
	"time"

	"github.com/sandertv/go-raknet"
)

func echo(conn net.Conn) {
	defer conn.Close()
	buffer := make([]byte, 65536)
	for {
		n, err := conn.Read(buffer)
		if err != nil {
			return
		}
		if _, err = conn.Write(buffer[:n]); err != nil {
			return
		}
	}
}

func main() {
	if len(os.Args) != 3 {
		panic("usage: interop-peer server|client address")
	}
	address := os.Args[2]
	if os.Args[1] == "server" {
		listener, err := (raknet.ListenConfig{DisableCookies: true, MaxMTU: 1400, ErrorLog: slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: slog.LevelDebug}))}).Listen(address)
		if err != nil {
			panic(err)
		}
		defer listener.Close()
		fmt.Println("READY", listener.Addr())
		for {
			conn, err := listener.Accept()
			if err != nil {
				panic(err)
			}
			go echo(conn)
		}
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	conn, err := raknet.DialContext(ctx, address)
	if err != nil {
		panic(err)
	}
	defer conn.Close()
	count := 0
	for _, size := range []int{64, 800, 4096, 10000} {
		for index := 0; index < 100; index++ {
			payload := make([]byte, size)
			for i := range payload {
				payload[i] = byte(i + index)
			}
			payload[0] = 0xfe
			if _, err = conn.Write(payload); err != nil {
				panic(err)
			}
			response, err := conn.ReadPacket()
			if err != nil {
				panic(err)
			}
			if !bytes.Equal(response, payload) {
				panic("message reordered, duplicated or corrupted")
			}
			count++
		}
	}
	fmt.Printf("PASS: %d unique ordered messages, 64/800/4096/10000 bytes\n", count)
}
