// Prints request payloads and frames built with the pinned Karabiner driver headers so the Rust
// client can be checked byte for byte (see scripts/vhid-reference.sh).
#include <cstdio>
#include <cstring>
#include <vector>
#include <pqrs/karabiner/driverkit/virtual_hid_device_driver.hpp>
#include <pqrs/karabiner/driverkit/virtual_hid_device_service.hpp>
#include <pqrs/unix_domain_stream/impl/protocol.hpp>

namespace service = pqrs::karabiner::driverkit::virtual_hid_device_service;
namespace reports = pqrs::karabiner::driverkit::virtual_hid_device_driver::hid_report;
namespace protocol = pqrs::unix_domain_stream::impl::protocol;

template <typename T>
static void append(std::vector<uint8_t>& buffer, const T& data) {
  auto size = buffer.size();
  buffer.resize(size + sizeof(data));
  std::memcpy(buffer.data() + size, &data, sizeof(data));
}

template <typename T>
static std::vector<uint8_t> payload(service::request request, const T& data) {
  std::vector<uint8_t> buffer;
  append(buffer, pqrs::karabiner::driverkit::client_protocol_version::embedded_client_protocol_version);
  append(buffer, request);
  append(buffer, data);
  return buffer;
}

static void dump(const char* name, const std::vector<uint8_t>& bytes) {
  printf("%s ", name);
  for (auto byte : bytes) printf("%02x", byte);
  printf("\n");
}

int main() {
  service::virtual_hid_keyboard_parameters parameters(
      pqrs::hid::vendor_id::value_t(0xfeed), pqrs::hid::product_id::value_t(0xa2ad),
      pqrs::hid::country_code::value_t(0));
  auto init = payload(service::request::virtual_hid_keyboard_initialize, parameters);
  dump("keyboard_initialize", init);
  dump("keyboard_initialize_frame", protocol::make_request_frame(0x0102030405060708, init));

  reports::keyboard_input keyboard;
  keyboard.modifiers.insert(reports::modifier::left_option);
  keyboard.keys.insert(0x2c);
  keyboard.keys.insert(0x152);
  dump("keyboard_input", payload(service::request::post_keyboard_input_report, keyboard));

  reports::consumer_input consumer;
  consumer.keys.insert(0xe9);
  dump("consumer_input", payload(service::request::post_consumer_input_report, consumer));

  reports::apple_vendor_top_case_input top_case;
  top_case.keys.insert(0x03);
  dump("apple_top_case_input",
        payload(service::request::post_apple_vendor_top_case_input_report, top_case));

  reports::apple_vendor_keyboard_input apple_keyboard;
  apple_keyboard.keys.insert(0x01);
  dump("apple_keyboard_input",
        payload(service::request::post_apple_vendor_keyboard_input_report, apple_keyboard));

  reports::generic_desktop_input generic_desktop;
  generic_desktop.keys.insert(0x9b);
  dump("generic_desktop_input",
       payload(service::request::post_generic_desktop_input_report, generic_desktop));

  std::vector<uint8_t> terminate;
  append(terminate, pqrs::karabiner::driverkit::client_protocol_version::embedded_client_protocol_version);
  append(terminate, service::request::virtual_hid_keyboard_terminate);
  dump("keyboard_terminate", terminate);
  dump("heartbeat_frame", protocol::make_heartbeat_frame());
  dump("response_frame", protocol::make_response_frame(42, {}));
}
