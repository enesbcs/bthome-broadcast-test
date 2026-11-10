//! System disk temperature from `drivetemp` or NVMe hwmon.
//!
//! The rung deliberately covers only the filesystem that hosts `/`. The block
//! device is resolved through `/proc/self/mountinfo`, then matched against the
//! `device` link of the `drivetemp`/`nvme` hwmon device. Device-mapper and MD
//! roots are followed through their `slaves`; USB devices are skipped.

use std::path::{Path, PathBuf};

use crate::provider::ProviderError;

use super::sysfs::{
    block_slaves, canonical, hwmon_channels, preferred_channel, read_millidegrees,
    root_device_number, HwmonChannel,
};

/// hwmon drivers that measure a block device.
const HWMON_NAMES: &[&str] = &["drivetemp", "nvme"];

/// Reads the system disk temperature in degrees Celsius.
pub fn read(root: &Path) -> Result<f64, ProviderError> {
    let (major, minor) = root_device_number(root, "/").ok_or_else(|| {
        ProviderError::Unavailable("cannot determine the system disk from mountinfo".to_string())
    })?;

    let link = root.join(format!("sys/dev/block/{major}:{minor}"));
    let block = canonical(&link).ok_or_else(|| {
        ProviderError::Unavailable(format!(
            "block device {major}:{minor} is not present in sysfs"
        ))
    })?;

    let candidates = collect_block_devices(&block);
    if candidates.is_empty() {
        return Err(ProviderError::Unavailable(
            "the system disk is a USB device".to_string(),
        ));
    }

    let channels = hwmon_channels(root);
    let mut matched: Vec<HwmonChannel> = Vec::new();
    for channel in channels
        .iter()
        .filter(|channel| HWMON_NAMES.contains(&channel.device.as_str()))
    {
        let Some(device) = canonical(&channel.dir.join("device")) else {
            continue;
        };
        if candidates
            .iter()
            .any(|candidate| candidate.starts_with(&device))
        {
            matched.push(channel.clone());
        }
    }

    if matched.is_empty() {
        return Err(ProviderError::Unavailable(
            "no system disk temperature sensor (drivetemp/nvme hwmon)".to_string(),
        ));
    }

    let preferred = preferred_channel(&matched).map(|channel| channel.input.clone());
    if let Some(input) = preferred {
        if let Ok(celsius) = read_millidegrees(&input) {
            return Ok(celsius);
        }
    }
    let mut last_error = None;
    for channel in matched {
        match read_millidegrees(&channel.input) {
            Ok(celsius) => return Ok(celsius),
            Err(error) => last_error = Some(error),
        }
    }

    Err(last_error.unwrap_or_else(|| {
        ProviderError::Io("the disk temperature sensor could not be read".to_string())
    }))
}

/// The root block device plus any device-mapper/MD slaves below it.
fn collect_block_devices(block: &Path) -> Vec<PathBuf> {
    let mut collected = Vec::new();
    let mut queue = vec![block.to_path_buf()];

    while let Some(path) = queue.pop() {
        if is_usb(&path) {
            continue;
        }
        queue.extend(block_slaves(&path));
        collected.push(path);
    }
    collected
}

/// Whether a sysfs device path belongs to the USB bus.
fn is_usb(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str().to_string_lossy().starts_with("usb"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::linux::sysfs::fixture::Fixture;

    #[test]
    fn matches_a_drivetemp_sensor() {
        let fixture = Fixture::new("disk-drivetemp");
        fixture.file(
            "proc/self/mountinfo",
            "31 1 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw\n",
        );
        let disk = fixture.dir("sys/devices/pci0000:00/ata1/host0/target0:0:0/0:0:0:0");
        fixture.dir("sys/devices/pci0000:00/ata1/host0/target0:0:0/0:0:0:0/block/sda/sda1");
        fixture.symlink("sys/dev/block/8:1", &disk.join("block/sda/sda1"));

        fixture.file("sys/class/hwmon/hwmon0/name", "drivetemp\n");
        fixture.file("sys/class/hwmon/hwmon0/temp1_input", "39000\n");
        fixture.symlink("sys/class/hwmon/hwmon0/device", &disk);

        assert_eq!(read(fixture.path()), Ok(39.0));
    }

    #[test]
    fn matches_an_nvme_controller() {
        let fixture = Fixture::new("disk-nvme");
        fixture.file(
            "proc/self/mountinfo",
            "31 1 259:1 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p1 rw\n",
        );
        let controller = fixture.dir("sys/devices/pci0000:00/nvme/nvme0");
        fixture.dir("sys/devices/pci0000:00/nvme/nvme0/nvme0n1/nvme0n1p1");
        fixture.symlink("sys/dev/block/259:1", &controller.join("nvme0n1/nvme0n1p1"));

        fixture.file("sys/class/hwmon/hwmon0/name", "nvme\n");
        fixture.file("sys/class/hwmon/hwmon0/temp1_input", "41000\n");
        fixture.symlink("sys/class/hwmon/hwmon0/device", &controller);

        assert_eq!(read(fixture.path()), Ok(41.0));
    }

    #[test]
    fn follows_device_mapper_slaves() {
        let fixture = Fixture::new("disk-dm");
        fixture.file(
            "proc/self/mountinfo",
            "31 1 253:0 / / rw,relatime shared:1 - ext4 /dev/mapper/root rw\n",
        );
        let physical = fixture.dir("sys/devices/pci0000:00/ata1/host0/target0:0:0/0:0:0:0");
        fixture.dir("sys/devices/pci0000:00/ata1/host0/target0:0:0/0:0:0:0/block/sda/sda1");
        let dm = fixture.dir("sys/devices/virtual/block/dm-0");
        fixture.dir("sys/devices/virtual/block/dm-0/slaves");
        fixture.symlink(
            "sys/devices/virtual/block/dm-0/slaves/sda",
            &physical.join("block/sda/sda1"),
        );
        fixture.symlink("sys/dev/block/253:0", &dm);

        fixture.file("sys/class/hwmon/hwmon0/name", "drivetemp\n");
        fixture.file("sys/class/hwmon/hwmon0/temp1_input", "36000\n");
        fixture.symlink("sys/class/hwmon/hwmon0/device", &physical);

        assert_eq!(read(fixture.path()), Ok(36.0));
    }

    #[test]
    fn reports_a_missing_disk_sensor() {
        let fixture = Fixture::new("disk-none");
        fixture.file(
            "proc/self/mountinfo",
            "31 1 8:1 / / rw - ext4 /dev/sda1 rw\n",
        );
        fixture.dir("sys/devices/pci0000:00/ata1/0:0:0:0/block/sda/sda1");
        fixture.symlink(
            "sys/dev/block/8:1",
            &fixture.dir("sys/devices/pci0000:00/ata1/0:0:0:0/block/sda/sda1"),
        );

        assert!(matches!(
            read(fixture.path()),
            Err(ProviderError::Unavailable(_))
        ));
    }
}
