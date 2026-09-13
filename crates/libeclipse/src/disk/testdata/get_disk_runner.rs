// what GET_DISK printed on GitHub's windows-latest runner
const RUNNER: &str = r#"{
    "Administrator":  true,
    "Disks":  [
                  {
                      "Number":  2,
                      "FriendlyName":  "Msft Virtual Disk",
                      "SerialNumber":  null,
                      "BusType":  "File Backed Virtual",
                      "Size":  23622320128,
                      "LogicalSectorSize":  512,
                      "IsBoot":  false,
                      "IsSystem":  false,
                      "IsReadOnly":  false,
                      "IsOffline":  false,
                      "PartitionStyle":  "RAW"
                  },
                  {
                      "Number":  0,
                      "FriendlyName":  "Msft Virtual Disk",
                      "SerialNumber":  null,
                      "BusType":  "SAS",
                      "Size":  161061273600,
                      "LogicalSectorSize":  512,
                      "IsBoot":  true,
                      "IsSystem":  true,
                      "IsReadOnly":  false,
                      "IsOffline":  false,
                      "PartitionStyle":  "GPT"
                  },
                  {
                      "Number":  1,
                      "FriendlyName":  "Msft Virtual Disk",
                      "SerialNumber":  null,
                      "BusType":  "SAS",
                      "Size":  161061273600,
                      "LogicalSectorSize":  512,
                      "IsBoot":  false,
                      "IsSystem":  false,
                      "IsReadOnly":  false,
                      "IsOffline":  false,
                      "PartitionStyle":  "MBR"
                  }
              ],
    "Partitions":  [
                       {
                           "DiskNumber":  0,
                           "PartitionNumber":  1,
                           "DriveLetter":  "\u0000",
                           "AccessPaths":  [
                                               null
                                           ],
                           "Size":  16777216,
                           "FileSystem":  null,
                           "Label":  null
                       },
                       {
                           "DiskNumber":  0,
                           "PartitionNumber":  2,
                           "DriveLetter":  "\u0000",
                           "AccessPaths":  [
                                               "\\\\?\\Volume{c310a8dc-799c-43d9-bdf5-dcd066eeaad4}\\"
                                           ],
                           "Size":  471859200,
                           "FileSystem":  "NTFS",
                           "Label":  "Recovery"
                       },
                       {
                           "DiskNumber":  0,
                           "PartitionNumber":  3,
                           "DriveLetter":  "\u0000",
                           "AccessPaths":  [
                                               "\\\\?\\Volume{1342b469-2fae-413d-9afd-da9d00b053ed}\\"
                                           ],
                           "Size":  103809024,
                           "FileSystem":  "FAT32",
                           "Label":  ""
                       },
                       {
                           "DiskNumber":  0,
                           "PartitionNumber":  4,
                           "DriveLetter":  "C",
                           "AccessPaths":  [
                                               "C:\\",
                                               "\\\\?\\Volume{efef19e4-aa31-40cb-91c7-6829a696271c}\\"
                                           ],
                           "Size":  160467762688,
                           "FileSystem":  "NTFS",
                           "Label":  "Windows"
                       },
                       {
                           "DiskNumber":  1,
                           "PartitionNumber":  1,
                           "DriveLetter":  "D",
                           "AccessPaths":  [
                                               "D:\\",
                                               "\\\\?\\Volume{c466e9e6-0000-0000-0000-100000000000}\\"
                                           ],
                           "Size":  161059176448,
                           "FileSystem":  "NTFS",
                           "Label":  "Temporary Storage"
                       }
                   ]
}
"#;
