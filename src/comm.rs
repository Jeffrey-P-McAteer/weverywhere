

/// Terminals are used to refer to single-hosts. They
/// have a single PKI identity and act as the verticies of network graphs
/// for communication purposes.
pub struct Terminal {
  pub human_name: String,

  pub human_name_utf8sig: Vec<u8>,

  pub pub_key: Vec<u8>,

  pub connection: Box<dyn Connectable>,

}


/// Abstraction over TCP, UDP, and Unix sockets.
/// A Ship represents a connection between 2 PCs.
/// Ships _may_ use UDP multicasting, but the identity information
/// must only ever refer to a single host to allow peers to correctly issue
/// parallel compute requests until needs are met.
pub struct Ship {

}



/// A Comm Lake is a group of ships, and when messages are sent to a lake
/// all ships receive the data.
pub struct Lake {
  pub ships: Vec<Ship>,
}


/// This trait allows us to implement TCP/UDP/Socket connections later without needing to
/// hoist storage for those connections in the comm structs defined here.
pub trait Connectable {
   fn connect(&mut self) -> Result<(), Box<dyn std::error::Error>>;

   fn is_connected(&mut self) -> Result<bool, Box<dyn std::error::Error>>;

   fn tx_encoded(&mut self, message: &[u8]) -> Result<(), Box<dyn std::error::Error>>;
   fn rx_encoded(&mut self) -> Result<(), Box<dyn std::error::Error>>;

   fn disconnect(&mut self) -> Result<(), Box<dyn std::error::Error>>;

}

