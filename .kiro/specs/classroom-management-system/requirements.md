# Requirements Document

## Introduction

Hệ thống Quản Lý Phòng Máy (Classroom Management System) - một giải pháp toàn diện cho phép giáo viên giám sát, điều khiển và quản lý các máy tính học sinh trong môi trường phòng lab/phòng máy.

## Glossary

- **Admin_App**: Ứng dụng chạy trên máy giáo viên để giám sát và điều khiển
- **Agent**: Phần mềm nhẹ chạy trên máy học sinh
- **Grid_View**: Chế độ hiển thị nhiều màn hình thu nhỏ cùng lúc
- **WOL**: Wake-on-LAN - giao thức đánh thức máy tính từ xa
- **mDNS**: Multicast DNS - giao thức tự động khám phá thiết bị trong mạng LAN
- **Dirty_Region**: Vùng màn hình có thay đổi cần cập nhật

## Requirements

### Requirement 1: Real-time Screen Monitoring

**User Story:** As a teacher, I want to view all student screens simultaneously, so that I can monitor their activities during class.

#### Acceptance Criteria

1. THE Admin_App SHALL display up to 50 student screens in Grid_View simultaneously
2. WHEN a student screen changes, THE Admin_App SHALL update the thumbnail within 500ms
3. THE Agent SHALL capture screen at configurable frame rate (1-30 FPS)
4. THE Agent SHALL use less than 5% CPU when capturing at 2 FPS
5. WHEN network bandwidth is limited, THE System SHALL automatically reduce frame rate

### Requirement 2: Remote Desktop Control

**User Story:** As a teacher, I want to remotely control a student's computer, so that I can assist them or demonstrate something.

#### Acceptance Criteria

1. WHEN teacher clicks on a student thumbnail, THE Admin_App SHALL open fullscreen remote view
2. THE Admin_App SHALL transmit mouse movements to the selected Agent
3. THE Admin_App SHALL transmit mouse clicks (left, right, middle) to the selected Agent
4. THE Admin_App SHALL transmit keyboard input to the selected Agent
5. WHEN remote control session ends, THE Agent SHALL restore normal input control

### Requirement 3: Screen Locking

**User Story:** As a teacher, I want to lock student screens, so that I can get their attention for announcements.

#### Acceptance Criteria

1. WHEN teacher activates lock, THE Agent SHALL display fullscreen lock overlay
2. WHILE screen is locked, THE Agent SHALL block all keyboard input except Ctrl+Alt+Del
3. WHILE screen is locked, THE Agent SHALL block all mouse input
4. THE Admin_App SHALL support locking individual machines or all machines
5. WHEN teacher unlocks, THE Agent SHALL restore normal operation within 100ms

### Requirement 4: Power Management

**User Story:** As a teacher, I want to remotely power on/off student computers, so that I can prepare the lab efficiently.

#### Acceptance Criteria

1. THE Admin_App SHALL send Wake-on-LAN packets to power on offline machines
2. THE Admin_App SHALL send shutdown command to power off machines
3. THE Admin_App SHALL send restart command to reboot machines
4. THE Admin_App SHALL send logout command to sign out current user
5. WHEN power command is sent, THE System SHALL confirm execution status

### Requirement 5: Remote Command Execution

**User Story:** As a teacher, I want to run programs or commands on student computers, so that I can open learning applications or close distracting programs.

#### Acceptance Criteria

1. THE Admin_App SHALL allow teacher to specify application path to launch
2. THE Agent SHALL execute the specified application on the student machine
3. THE Admin_App SHALL allow teacher to terminate running processes by name
4. THE Agent SHALL report success/failure of command execution
5. THE System SHALL maintain a list of commonly used commands for quick access

### Requirement 6: File Transfer

**User Story:** As a teacher, I want to send files to all students at once, so that I can distribute learning materials efficiently.

#### Acceptance Criteria

1. THE Admin_App SHALL allow selecting files to broadcast to all Agents
2. THE System SHALL compress files before transfer using efficient compression
3. THE System SHALL transfer files in parallel to multiple Agents
4. THE Agent SHALL save received files to a configurable download folder
5. WHEN transfer completes, THE System SHALL report success/failure for each Agent

### Requirement 7: Auto-Discovery (Zero Configuration)

**User Story:** As a teacher, I want student computers to automatically connect, so that I don't need to configure IP addresses manually.

#### Acceptance Criteria

1. WHEN Agent starts, THE Agent SHALL broadcast its presence via mDNS
2. THE Admin_App SHALL automatically discover all Agents on the local network
3. WHEN a new Agent joins, THE Admin_App SHALL add it to the machine list within 5 seconds
4. WHEN an Agent disconnects, THE Admin_App SHALL mark it as offline within 10 seconds
5. THE System SHALL work without requiring static IP configuration

### Requirement 8: Lightweight Agent

**User Story:** As a system administrator, I want the agent software to be lightweight, so that it doesn't affect student computer performance.

#### Acceptance Criteria

1. THE Agent SHALL use less than 15 MB RAM during normal operation
2. THE Agent installer SHALL be less than 5 MB in size
3. THE Agent SHALL start automatically with Windows
4. THE Agent SHALL run as a background service without visible window
5. THE Agent SHALL reconnect automatically if connection is lost

### Requirement 9: Security

**User Story:** As a system administrator, I want secure communication, so that students cannot intercept or spoof commands.

#### Acceptance Criteria

1. THE System SHALL encrypt all network communication using TLS
2. THE Agent SHALL authenticate the Admin_App before accepting commands
3. THE Admin_App SHALL require password authentication to access
4. THE System SHALL log all administrative actions for audit purposes
5. IF authentication fails, THEN THE Agent SHALL reject the connection

## Priority Order

1. **Phase 1 (MVP)**: Requirements 1, 2, 3 - Core monitoring and control
2. **Phase 2**: Requirements 4, 5 - Power and command management
3. **Phase 3**: Requirements 6, 7 - File transfer and auto-discovery
4. **Phase 4**: Requirements 8, 9 - Optimization and security
