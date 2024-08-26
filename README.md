
  <h1>File Transfer Application</h1>

  <h3>Overview</h3>
  <strong>This file transfer application, crafted in Rust and powered by the tokio library, facilitates seamless file exchange over TCP connections. Designed to operate in dual modes—send and receive—it ensures efficient and reliable transmission of files.</strong>
  <strong>It supports sending and receiving files over TCP connections.</strong>
  <strong>The application can operate in two modes: `send` and `receive`.</strong>

  <h3>Prerequisites</h3>
<strong>Before you start, ensure you have the following installed:</strong>
<ul>
    <li><strong>Rust programming language (version 1.65 or higher)</strong></li>
    <li><strong>tokio library (async runtime for Rust)</strong></li>
    <li><strong>clap library (for command-line argument parsing)</strong></li>
    <li><strong>Ngrok (for exposing local servers to the internet)</strong></li>
</ul>


  <h3>Setup</h3>
  <strong>To set up the project, follow these steps:</strong>
  <ol>
      <li><strong>Clone the repository:</strong> <code>git clone https://github.com/idevanshu/p2pfiletransferRust.git</code></li>
      <li><strong>Navigate to the project directory:</strong> <code>cd p2pfiletransferRust</code></li>
      <li><strong>Build the project:</strong> <code>cargo build</code></li>
  </ol>

  <h3>Example Usage</h3>
<strong>Sending a file:</strong>
<pre><code>cargo run -- --mode send --file /path/to/file.txt
</code></pre>
<pre><code>ngrok tcp 8000
</code></pre>
<strong>Receiving a file:</strong>
<pre><code>cargo run -- --mode receive --address 0.tcp.ngrok.io:18944
</code></pre>

<em>Note:</em> Replace `0.tcp.ngrok.io:18944` with the actual Ngrok address and port shown in the Ngrok output.


   
