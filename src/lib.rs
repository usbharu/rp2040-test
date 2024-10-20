/*

https://github.com/embassy-rs/embassy

*/

#![no_std]

use core::fmt::Write as _;

use embassy_futures::join::join;
use embassy_sync::pipe::Pipe;
use embassy_usb::class::cdc_acm::{CdcAcmClass, Receiver, Sender, State};
use embassy_usb::driver::Driver;
use embassy_usb::{Builder, Config};
use log::{Metadata, Record};
type CS = embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

/// The logger state containing buffers that must live as long as the USB peripheral.
pub struct LoggerState<'d> {
    state: State<'d>,
    config_descriptor: [u8; 128],
    bos_descriptor: [u8; 16],
    msos_descriptor: [u8; 256],
    control_buf: [u8; 64],
}

impl<'d> LoggerState<'d> {
    /// Create a new instance of the logger state.
    pub fn new() -> Self {
        Self {
            state: State::new(),
            config_descriptor: [0; 128],
            bos_descriptor: [0; 16],
            msos_descriptor: [0; 256],
            control_buf: [0; 64],
        }
    }
}

/// The packet size used in the usb logger, to be used with `create_future_from_class`
pub const MAX_PACKET_SIZE: u8 = 64;

/// The logger handle, which contains a pipe with configurable size for buffering log messages.
pub struct UsbLogger<const N: usize> {
    buffer: Pipe<CS, N>,
    custom_style: Option<fn(&Record, &mut Writer<'_, N>) -> ()>,
}

impl<const N: usize> UsbLogger<N> {
    /// Create a new logger instance.
    pub const fn new() -> Self {
        Self {
            buffer: Pipe::new(),
            custom_style: None,
        }
    }

    /// Create a new logger instance with a custom formatter.
    pub const fn with_custom_style(custom_style: fn(&Record, &mut Writer<'_, N>) -> ()) -> Self {
        Self {
            buffer: Pipe::new(),
            custom_style: Some(custom_style),
        }
    }

    /// Run the USB logger using the state and USB driver. Never returns.
    pub async fn run<'d, D>(&'d self, state: &'d mut LoggerState<'d>, driver: D) -> !
    where
        D: Driver<'d>,
        Self: 'd,
    {
        let mut config = Config::new(0xc0de, 0xcafe);
        config.manufacturer = Some("Embassy");
        config.product = Some("USB-serial logger");
        config.serial_number = None;
        config.max_power = 100;
        config.max_packet_size_0 = MAX_PACKET_SIZE;

        // Required for windows compatiblity.
        // https://developer.nordicsemi.com/nRF_Connect_SDK/doc/1.9.1/kconfig/CONFIG_CDC_ACM_IAD.html#help
        config.device_class = 0xEF;
        config.device_sub_class = 0x02;
        config.device_protocol = 0x01;
        config.composite_with_iads = true;

        let mut builder = Builder::new(
            driver,
            config,
            &mut state.config_descriptor,
            &mut state.bos_descriptor,
            &mut state.msos_descriptor,
            &mut state.control_buf,
        );

        // Create classes on the builder.
        let class = CdcAcmClass::new(&mut builder, &mut state.state, MAX_PACKET_SIZE as u16);
        let (mut sender, mut receiver) = class.split();

        // Build the builder.
        let mut device = builder.build();
        loop {
            let run_fut = device.run();
            let class_fut = self.run_logger_class(&mut sender, &mut receiver);
            join(run_fut, class_fut).await;
        }
    }

    async fn run_logger_class<'d, D>(
        &self,
        sender: &mut Sender<'d, D>,
        receiver: &mut Receiver<'d, D>,
    ) where
        D: Driver<'d>,
    {
        let log_fut = async {
            let mut rx: [u8; MAX_PACKET_SIZE as usize] = [0; MAX_PACKET_SIZE as usize];
            sender.wait_connection().await;
            loop {
                let len = self.buffer.read(&mut rx[..]).await;
                let _ = sender.write_packet(&rx[..len]).await;
                if len as u8 == MAX_PACKET_SIZE {
                    let _ = sender.write_packet(&[]).await;
                }
            }
        };
        let discard_fut = async {
            let mut discard_buf: [u8; MAX_PACKET_SIZE as usize] = [0; MAX_PACKET_SIZE as usize];
            receiver.wait_connection().await;
            loop {
                let _ = receiver.read_packet(&mut discard_buf).await;
            }
        };

        join(log_fut, discard_fut).await;
    }

    /// Creates the futures needed for the logger from a given class
    /// This can be used in cases where the usb device is already in use for another connection
    pub async fn create_future_from_class<'d, D>(&'d self, class: CdcAcmClass<'d, D>)
    where
        D: Driver<'d>,
    {
        let (mut sender, mut receiver) = class.split();
        loop {
            self.run_logger_class(&mut sender, &mut receiver).await;
        }
    }
}

impl<const N: usize> log::Log for UsbLogger<N> {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            if let Some(custom_style) = self.custom_style {
                custom_style(record, &mut Writer(&self.buffer));
            } else {
                let _ = write!(Writer(&self.buffer), "{}\r\n", record.args());
            }
        }
    }

    fn flush(&self) {}
}

/// A writer that writes to the USB logger buffer.
pub struct Writer<'d, const N: usize>(&'d Pipe<CS, N>);

impl<'d, const N: usize> core::fmt::Write for Writer<'d, N> {
    fn write_str(&mut self, s: &str) -> Result<(), core::fmt::Error> {
        // The Pipe is implemented in such way that we cannot
        // write across the wraparound discontinuity.
        let b = s.as_bytes();
        if let Ok(n) = self.0.try_write(b) {
            if n < b.len() {
                // We wrote some data but not all, attempt again
                // as the reason might be a wraparound in the
                // ring buffer, which resolves on second attempt.
                let _ = self.0.try_write(&b[n..]);
            }
        }
        Ok(())
    }
}

/// Initialize and run the USB serial logger, never returns.
///
/// Arguments specify the buffer size, log level and the USB driver, respectively.
///
/// # Usage
///
/// ```
/// embassy_usb_logger::run!(1024, log::LevelFilter::Info, driver);
/// ```
///
/// # Safety
///
/// This macro should only be invoked only once since it is setting the global logging state of the application.
#[macro_export]
macro_rules! run {
    ( $x:expr, $l:expr, $p:ident ) => {
        static LOGGER: ::rp2040_project_template::UsbLogger<$x> =
            ::rp2040_project_template::UsbLogger::new();
        unsafe {
            let _ = ::log::set_logger_racy(&LOGGER).map(|()| log::set_max_level_racy($l));
        }
        let _ = LOGGER
            .run(&mut ::rp2040_project_template::LoggerState::new(), $p)
            .await;
    };
}

/// Initialize the USB serial logger from a serial class and return the future to run it.
///
/// Arguments specify the buffer size, log level and the serial class, respectively.
///
/// # Usage
///
/// ```
/// embassy_usb_logger::with_class!(1024, log::LevelFilter::Info, class);
/// ```
///
/// # Safety
///
/// This macro should only be invoked only once since it is setting the global logging state of the application.
#[macro_export]
macro_rules! with_class {
    ( $x:expr, $l:expr, $p:ident ) => {{
        static LOGGER: ::embassy_usb_logger::UsbLogger<$x> = ::embassy_usb_logger::UsbLogger::new();
        unsafe {
            let _ = ::log::set_logger_racy(&LOGGER).map(|()| log::set_max_level_racy($l));
        }
        LOGGER.create_future_from_class($p)
    }};
}

/// Initialize the USB serial logger from a serial class and return the future to run it.
/// This version of the macro allows for a custom style function to be passed in.
/// The custom style function will be called for each log record and is responsible for writing the log message to the buffer.
///
/// Arguments specify the buffer size, log level, the serial class and the custom style function, respectively.
///
/// # Usage
///
/// ```
/// let log_fut = embassy_usb_logger::with_custom_style!(1024, log::LevelFilter::Info, logger_class, |record, writer| {
///     use core::fmt::Write;
///     let level = record.level().as_str();
///     write!(writer, "[{level}] {}\r\n", record.args()).unwrap();
/// });
/// ```
///
/// # Safety
///
/// This macro should only be invoked only once since it is setting the global logging state of the application.
#[macro_export]
macro_rules! with_custom_style {
    ( $x:expr, $l:expr, $p:ident, $s:expr ) => {{
        static LOGGER: ::embassy_usb_logger::UsbLogger<$x> =
            ::embassy_usb_logger::UsbLogger::with_custom_style($s);
        unsafe {
            let _ = ::log::set_logger_racy(&LOGGER).map(|()| log::set_max_level_racy($l));
        }
        LOGGER.create_future_from_class($p)
    }};
}

/*

                              Apache License
                        Version 2.0, January 2004
                     http://www.apache.org/licenses/

TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION

1. Definitions.

   "License" shall mean the terms and conditions for use, reproduction,
   and distribution as defined by Sections 1 through 9 of this document.

   "Licensor" shall mean the copyright owner or entity authorized by
   the copyright owner that is granting the License.

   "Legal Entity" shall mean the union of the acting entity and all
   other entities that control, are controlled by, or are under common
   control with that entity. For the purposes of this definition,
   "control" means (i) the power, direct or indirect, to cause the
   direction or management of such entity, whether by contract or
   otherwise, or (ii) ownership of fifty percent (50%) or more of the
   outstanding shares, or (iii) beneficial ownership of such entity.

   "You" (or "Your") shall mean an individual or Legal Entity
   exercising permissions granted by this License.

   "Source" form shall mean the preferred form for making modifications,
   including but not limited to software source code, documentation
   source, and configuration files.

   "Object" form shall mean any form resulting from mechanical
   transformation or translation of a Source form, including but
   not limited to compiled object code, generated documentation,
   and conversions to other media types.

   "Work" shall mean the work of authorship, whether in Source or
   Object form, made available under the License, as indicated by a
   copyright notice that is included in or attached to the work
   (an example is provided in the Appendix below).

   "Derivative Works" shall mean any work, whether in Source or Object
   form, that is based on (or derived from) the Work and for which the
   editorial revisions, annotations, elaborations, or other modifications
   represent, as a whole, an original work of authorship. For the purposes
   of this License, Derivative Works shall not include works that remain
   separable from, or merely link (or bind by name) to the interfaces of,
   the Work and Derivative Works thereof.

   "Contribution" shall mean any work of authorship, including
   the original version of the Work and any modifications or additions
   to that Work or Derivative Works thereof, that is intentionally
   submitted to Licensor for inclusion in the Work by the copyright owner
   or by an individual or Legal Entity authorized to submit on behalf of
   the copyright owner. For the purposes of this definition, "submitted"
   means any form of electronic, verbal, or written communication sent
   to the Licensor or its representatives, including but not limited to
   communication on electronic mailing lists, source code control systems,
   and issue tracking systems that are managed by, or on behalf of, the
   Licensor for the purpose of discussing and improving the Work, but
   excluding communication that is conspicuously marked or otherwise
   designated in writing by the copyright owner as "Not a Contribution."

   "Contributor" shall mean Licensor and any individual or Legal Entity
   on behalf of whom a Contribution has been received by Licensor and
   subsequently incorporated within the Work.

2. Grant of Copyright License. Subject to the terms and conditions of
   this License, each Contributor hereby grants to You a perpetual,
   worldwide, non-exclusive, no-charge, royalty-free, irrevocable
   copyright license to reproduce, prepare Derivative Works of,
   publicly display, publicly perform, sublicense, and distribute the
   Work and such Derivative Works in Source or Object form.

3. Grant of Patent License. Subject to the terms and conditions of
   this License, each Contributor hereby grants to You a perpetual,
   worldwide, non-exclusive, no-charge, royalty-free, irrevocable
   (except as stated in this section) patent license to make, have made,
   use, offer to sell, sell, import, and otherwise transfer the Work,
   where such license applies only to those patent claims licensable
   by such Contributor that are necessarily infringed by their
   Contribution(s) alone or by combination of their Contribution(s)
   with the Work to which such Contribution(s) was submitted. If You
   institute patent litigation against any entity (including a
   cross-claim or counterclaim in a lawsuit) alleging that the Work
   or a Contribution incorporated within the Work constitutes direct
   or contributory patent infringement, then any patent licenses
   granted to You under this License for that Work shall terminate
   as of the date such litigation is filed.

4. Redistribution. You may reproduce and distribute copies of the
   Work or Derivative Works thereof in any medium, with or without
   modifications, and in Source or Object form, provided that You
   meet the following conditions:

   (a) You must give any other recipients of the Work or
       Derivative Works a copy of this License; and

   (b) You must cause any modified files to carry prominent notices
       stating that You changed the files; and

   (c) You must retain, in the Source form of any Derivative Works
       that You distribute, all copyright, patent, trademark, and
       attribution notices from the Source form of the Work,
       excluding those notices that do not pertain to any part of
       the Derivative Works; and

   (d) If the Work includes a "NOTICE" text file as part of its
       distribution, then any Derivative Works that You distribute must
       include a readable copy of the attribution notices contained
       within such NOTICE file, excluding those notices that do not
       pertain to any part of the Derivative Works, in at least one
       of the following places: within a NOTICE text file distributed
       as part of the Derivative Works; within the Source form or
       documentation, if provided along with the Derivative Works; or,
       within a display generated by the Derivative Works, if and
       wherever such third-party notices normally appear. The contents
       of the NOTICE file are for informational purposes only and
       do not modify the License. You may add Your own attribution
       notices within Derivative Works that You distribute, alongside
       or as an addendum to the NOTICE text from the Work, provided
       that such additional attribution notices cannot be construed
       as modifying the License.

   You may add Your own copyright statement to Your modifications and
   may provide additional or different license terms and conditions
   for use, reproduction, or distribution of Your modifications, or
   for any such Derivative Works as a whole, provided Your use,
   reproduction, and distribution of the Work otherwise complies with
   the conditions stated in this License.

5. Submission of Contributions. Unless You explicitly state otherwise,
   any Contribution intentionally submitted for inclusion in the Work
   by You to the Licensor shall be under the terms and conditions of
   this License, without any additional terms or conditions.
   Notwithstanding the above, nothing herein shall supersede or modify
   the terms of any separate license agreement you may have executed
   with Licensor regarding such Contributions.

6. Trademarks. This License does not grant permission to use the trade
   names, trademarks, service marks, or product names of the Licensor,
   except as required for reasonable and customary use in describing the
   origin of the Work and reproducing the content of the NOTICE file.

7. Disclaimer of Warranty. Unless required by applicable law or
   agreed to in writing, Licensor provides the Work (and each
   Contributor provides its Contributions) on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or
   implied, including, without limitation, any warranties or conditions
   of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A
   PARTICULAR PURPOSE. You are solely responsible for determining the
   appropriateness of using or redistributing the Work and assume any
   risks associated with Your exercise of permissions under this License.

8. Limitation of Liability. In no event and under no legal theory,
   whether in tort (including negligence), contract, or otherwise,
   unless required by applicable law (such as deliberate and grossly
   negligent acts) or agreed to in writing, shall any Contributor be
   liable to You for damages, including any direct, indirect, special,
   incidental, or consequential damages of any character arising as a
   result of this License or out of the use or inability to use the
   Work (including but not limited to damages for loss of goodwill,
   work stoppage, computer failure or malfunction, or any and all
   other commercial damages or losses), even if such Contributor
   has been advised of the possibility of such damages.

9. Accepting Warranty or Additional Liability. While redistributing
   the Work or Derivative Works thereof, You may choose to offer,
   and charge a fee for, acceptance of support, warranty, indemnity,
   or other liability obligations and/or rights consistent with this
   License. However, in accepting such obligations, You may act only
   on Your own behalf and on Your sole responsibility, not on behalf
   of any other Contributor, and only if You agree to indemnify,
   defend, and hold each Contributor harmless for any liability
   incurred by, or claims asserted against, such Contributor by reason
   of your accepting any such warranty or additional liability.

END OF TERMS AND CONDITIONS

APPENDIX: How to apply the Apache License to your work.

   To apply the Apache License to your work, attach the following
   boilerplate notice, with the fields enclosed by brackets "[]"
   replaced with your own identifying information. (Don't include
   the brackets!)  The text should be enclosed in the appropriate
   comment syntax for the file format. We also recommend that a
   file or class name and description of purpose be included on the
   same "printed page" as the copyright notice for easier
   identification within third-party archives.

Copyright (c) Embassy project contributors

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.

*/
