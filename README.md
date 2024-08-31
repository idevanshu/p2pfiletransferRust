
  <h1>File Transfer Application</h1>

  <h3>Overview</h3>
  <strong>This file transfer application, crafted in Rust and powered by the tokio library, facilitates seamless file exchange over TCP connections. Designed to operate in dual modes—send and receive—it ensures efficient and reliable transmission of files.</strong>
  <strong>It supports sending and receiving files over TCP connections.</strong>
  <strong>The application can operate in two modes: `send` and `receive`.</strong>

  <h3>Prerequisites</h3>
<strong>Before you start, ensure you have the following installed:</strong>
<ul>
    <li><strong>Rust Programming Language (version 1.65 or higher)</strong></li>
    <li><strong>Tokio Library</strong> (for asynchronous runtime in Rust)</li>
    <li><strong>Clap Library</strong> (for command-line argument parsing)</li>
    <li>
    <strong>Port Forwarding Configuration</strong>
    <p>If you are using NAT, please configure port forwarding on your router:</p>
    <pre><code>TCP Port: 8000</code></pre>
</li>
    <li><strong>Ngrok</strong> (If your ISP employs CGNat, use Ngrok to expose local servers to the internet)</li>
</ul>


  <h3>Setup</h3>
  <strong>To set up the project, follow these steps:</strong>
  <ol>
      <li><strong>Clone the repository:</strong> <code>git clone https://github.com/idevanshu/p2pfiletransferRust.git</code></li>
      <li><strong>Navigate to the project directory:</strong> <code>cd p2pfiletransferRust</code></li>
      <li><strong>Build the project:</strong> <code>cargo build</code></li>
  </ol>

<h3>Example Usage</h3>

<h3>With Port Forwarding on Port 8000</h3>
<strong>To Send a File:</strong>
<pre><code>cargo run -- --mode send --file /path/to/file.txt
</code></pre>
<pre><code>ngrok tcp 8000
</code></pre>

<strong>To Receive a File:</strong>
<pre><code>cargo run -- --mode receive --address Your_Sender_Ip_Address:8000
</code></pre>

<h3>With Ngrok</h3>
<strong>To Send a File:</strong>
<pre><code>cargo run -- --mode send --file /path/to/file.txt
</code></pre>
<pre><code>ngrok tcp 8000
</code></pre>

<strong>To Receive a File:</strong>
<pre><code>cargo run -- --mode receive --address 0.tcp.ngrok.io:18944
</code></pre>

<em>Note:</em> Please replace `0.tcp.ngrok.io:18944` with the actual Ngrok address and port provided in the Ngrok output.



   
