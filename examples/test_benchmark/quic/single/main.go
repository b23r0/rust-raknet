package main

import (
	"bytes"
	"context"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/tls"
	"crypto/x509"
	"encoding/binary"
	"fmt"
	quic "github.com/quic-go/quic-go"
	"golang.org/x/sys/unix"
	"io"
	"math/big"
	"net"
	"os"
	"sort"
	"strconv"
	"sync"
	"syscall"
	"time"
)

func must(err error) {
	if err != nil {
		panic(err)
	}
}
func config() *quic.Config {
	return &quic.Config{InitialPacketSize: 1400, DisablePathMTUDiscovery: true, MaxIdleTimeout: 120 * time.Second, HandshakeIdleTimeout: 10 * time.Second, MaxIncomingStreams: 1}
}
func cert() tls.Certificate {
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	must(err)
	template := x509.Certificate{SerialNumber: big.NewInt(1), NotBefore: time.Now().Add(-time.Hour), NotAfter: time.Now().Add(time.Hour), DNSNames: []string{"localhost"}, KeyUsage: x509.KeyUsageDigitalSignature, ExtKeyUsage: []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth}}
	der, err := x509.CreateCertificate(rand.Reader, &template, &template, &key.PublicKey, key)
	must(err)
	return tls.Certificate{Certificate: [][]byte{der}, PrivateKey: key}
}
func writeRecord(s io.Writer, payload []byte, record []byte) {
	binary.LittleEndian.PutUint32(record[:4], uint32(len(payload)))
	copy(record[4:], payload)
	_, err := s.Write(record)
	must(err)
}
func readRecord(s io.Reader, actual []byte, header []byte) {
	_, err := io.ReadFull(s, header)
	must(err)
	if int(binary.LittleEndian.Uint32(header)) != len(actual) {
		panic("record size mismatch")
	}
	_, err = io.ReadFull(s, actual)
	must(err)
}
func acceptLoop(l *quic.Listener) {
	for {
		c, err := l.Accept(context.Background())
		must(err)
		go func() {
			defer c.CloseWithError(0, "")
			s, err := c.AcceptStream(context.Background())
			if err != nil {
				return
			}
			header := make([]byte, 4)
			record := make([]byte, 0)
			for {
				_, err = io.ReadFull(s, header)
				if err != nil {
					return
				}
				n := int(binary.LittleEndian.Uint32(header))
				if n > 65536 {
					return
				}
				if cap(record) < n+4 {
					record = make([]byte, n+4)
				} else {
					record = record[:n+4]
				}
				copy(record, header)
				_, err = io.ReadFull(s, record[4:])
				if err != nil {
					return
				}
				_, err = s.Write(record)
				if err != nil {
					return
				}
			}
		}()
	}
}
func server(address string) {
	tlsConfig := &tls.Config{Certificates: []tls.Certificate{cert()}, NextProtos: []string{"echo-bench"}}
	lc := net.ListenConfig{Control: func(network, address string, raw syscall.RawConn) error {
		var err error
		e := raw.Control(func(fd uintptr) { err = unix.SetsockoptInt(int(fd), unix.SOL_SOCKET, unix.SO_REUSEPORT, 1) })
		if e != nil {
			return e
		}
		return err
	}}
	for i := 0; i < 1; i++ {
		p, err := lc.ListenPacket(context.Background(), "udp4", address)
		must(err)
		l, err := quic.Listen(p, tlsConfig, config())
		must(err)
		go acceptLoop(l)
	}
	fmt.Println("QUIC echo listening", address, "with one receive socket")
	select {}
}

type peer struct {
	c  *quic.Conn
	s  *quic.Stream
	id int
}

func client(address string, count, messages, size int) {
	ctx, cancel := context.WithTimeout(context.Background(), 180*time.Second)
	defer cancel()
	setup := time.Now()
	peers := make([]peer, count)
	dial := make(chan struct{}, 8)
	var wg sync.WaitGroup
	for id := 0; id < count; id++ {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()
			dial <- struct{}{}
			defer func() { <-dial }()
			c, err := quic.DialAddr(ctx, address, &tls.Config{InsecureSkipVerify: true, NextProtos: []string{"echo-bench"}}, config())
			must(err)
			s, err := c.OpenStreamSync(ctx)
			must(err)
			must(s.SetDeadline(time.Now().Add(180 * time.Second)))
			peers[id] = peer{c, s, id}
		}(id)
	}
	wg.Wait()
	setupSeconds := time.Since(setup).Seconds()
	ready := make(chan struct{}, count)
	start := make(chan struct{})
	values := make([][]int64, count)
	for _, p := range peers {
		wg.Add(1)
		go func(p peer) {
			defer wg.Done()
			out := bytes.Repeat([]byte{0xfe}, size)
			binary.LittleEndian.PutUint64(out[1:9], uint64(p.id))
			expected := bytes.Clone(out)
			actual := make([]byte, size)
			record := make([]byte, size+4)
			header := make([]byte, 4)
			rtts := make([]int64, 20)
			for m := 0; m < 20; m++ {
				binary.LittleEndian.PutUint64(out[9:17], uint64(m))
				t := time.Now()
				writeRecord(p.s, out, record)
				readRecord(p.s, actual, header)
				if !bytes.Equal(actual, out) {
					panic("warmup mismatch")
				}
				rtts[m] = time.Since(t).Nanoseconds()
			}
			values[p.id] = rtts
			ready <- struct{}{}
			<-start
			sent := 0
			for sent < min(messages, 16) {
				binary.LittleEndian.PutUint64(out[9:17], uint64(sent+20))
				writeRecord(p.s, out, record)
				sent++
			}
			for received := 0; received < messages; received++ {
				binary.LittleEndian.PutUint64(expected[9:17], uint64(received+20))
				readRecord(p.s, actual, header)
				if !bytes.Equal(actual, expected) {
					panic("echo payload or order mismatch")
				}
				if sent < messages {
					binary.LittleEndian.PutUint64(out[9:17], uint64(sent+20))
					writeRecord(p.s, out, record)
					sent++
				}
			}
		}(p)
	}
	for i := 0; i < count; i++ {
		<-ready
	}
	t := time.Now()
	close(start)
	wg.Wait()
	elapsed := time.Since(t).Seconds()
	rtts := make([]int64, 0, count*20)
	for _, v := range values {
		rtts = append(rtts, v...)
	}
	sort.Slice(rtts, func(i, j int) bool { return rtts[i] < rtts[j] })
	percentile := func(p int) float64 { return float64(rtts[(len(rtts)*p+99)/100-1]) / 1000 }
	fmt.Printf("Connections: %d\nMessages per connection: %d\nPayload size: %d bytes\nSetup: %.6f s\nElapsed: %.6f s\nEcho payload throughput (per direction): %.2f MiB/s\nRTT p50: %.1f us\nRTT p95: %.1f us\nRTT p99: %.1f us\nVerified ordered echoes: %d\n", count, messages, size, setupSeconds, elapsed, float64(count)*float64(messages)*float64(size)/1048576/elapsed, percentile(50), percentile(95), percentile(99), count*(messages+20))
	for _, p := range peers {
		p.c.CloseWithError(0, "")
	}
}
func single(address string, messages, size, warmups, samples int) {
	ctx, cancel := context.WithTimeout(context.Background(), 240*time.Second)
	defer cancel()
	c, err := quic.DialAddr(ctx, address, &tls.Config{InsecureSkipVerify: true, NextProtos: []string{"echo-bench"}}, config())
	must(err)
	defer c.CloseWithError(0, "")
	stream, err := c.OpenStreamSync(ctx)
	must(err)
	must(stream.SetDeadline(time.Now().Add(240 * time.Second)))
	payload := bytes.Repeat([]byte{0xfe}, size)
	record := make([]byte, size+4)
	binary.LittleEndian.PutUint32(record[:4], uint32(size))
	copy(record[4:], payload)
	actual := make([]byte, size)
	header := make([]byte, 4)
	round := func() {
		_, err := stream.Write(record)
		must(err)
		readRecord(stream, actual, header)
		if !bytes.Equal(actual, payload) {
			panic("echo mismatch")
		}
	}
	for i := 0; i < warmups; i++ {
		round()
	}
	rtts := make([]int64, samples)
	for i := 0; i < samples; i++ {
		t := time.Now()
		round()
		rtts[i] = time.Since(t).Nanoseconds()
	}
	t := time.Now()
	slots := make(chan struct{}, 64)
	for i := 0; i < 64; i++ {
		slots <- struct{}{}
	}
	done := make(chan struct{})
	go func() {
		defer close(done)
		for i := 0; i < messages; i++ {
			<-slots
			_, err := stream.Write(record)
			must(err)
		}
	}()
	for i := 0; i < messages; i++ {
		readRecord(stream, actual, header)
		if !bytes.Equal(actual, payload) {
			panic("burst mismatch")
		}
		slots <- struct{}{}
	}
	<-done
	elapsed := time.Since(t).Seconds()
	sort.Slice(rtts, func(i, j int) bool { return rtts[i] < rtts[j] })
	percentile := func(p int) float64 { return float64(rtts[(samples*p+99)/100-1]) / 1000 }
	fmt.Printf("Elapsed: %.6f s\nEcho payload throughput (per direction): %.2f MiB/s\nRTT p50: %.1f us\nRTT p95: %.1f us\nRTT p99: %.1f us\nVerified ordered echoes: %d\n", elapsed, float64(messages)*float64(size)/1048576/elapsed, percentile(50), percentile(95), percentile(99), messages+warmups+samples)
}
func main() {
	if len(os.Args) == 3 && os.Args[1] == "server" {
		server(os.Args[2])
		return
	}
	if len(os.Args) != 7 {
		panic("single ADDRESS MESSAGES PAYLOAD WARMUP SAMPLES")
	}
	var values [4]int
	for i := range values {
		v, err := strconv.Atoi(os.Args[3+i])
		must(err)
		values[i] = v
	}
	single(os.Args[2], values[0], values[1], values[2], values[3])
}
